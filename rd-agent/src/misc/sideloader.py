#!/bin/env python3
# Copyright (c) Facebook, Inc. and its affiliates

import argparse
import os
import sys
import time
import math
import pathlib
import json
import re
import subprocess
import datetime
import signal
import multiprocessing
import subprocess
import platform

# Iterate every second
interval = 1

USER_HZ = os.sysconf(os.sysconf_names['SC_CLK_TCK'])
CGRP_BASE = '/sys/fs/cgroup'
SL_BASE = '/var/lib/sideloader'
SVC_SUFFIX = '.service'
dfl_cfg_file = SL_BASE + '/config.json'
dfl_job_dir = SL_BASE + '/jobs.d'
dfl_status_file = SL_BASE + '/status.json'
dfl_scribe_file = SL_BASE + '/scribe.json'
dfl_svc_prefix = 'sideload-'
systemd_root_override_file = '/etc/systemd/system/-.slice.d/zz-sideloader-disable-controller-override.conf'

description = '''
Resource control side-workload manager. See the following for details.

   https://fb.quip.com/qYC2Ay7SyyO7

'''

parser = argparse.ArgumentParser(formatter_class=argparse.RawDescriptionHelpFormatter,
                                 description=description)
parser.add_argument('--config', default=dfl_cfg_file,
                    help='Config file (default: %(default)s)')
parser.add_argument('--jobdir', default=dfl_job_dir,
                    help='Job input directory (default: %(default)s)')
parser.add_argument('--status', default=dfl_status_file,
                    help='Status file (default: %(default)s)')
parser.add_argument('--scribe', default=dfl_scribe_file,
                    help='Scribe input file (default: %(default)s)')
parser.add_argument('--svc-prefix', default=dfl_svc_prefix,
                    help='Sideload service name prefix (default: %(default)s)')
parser.add_argument('--dev', metavar='DEV',
                    help='Storage device detection override (e.g. sda, nvme0n1)')
parser.add_argument('--dont-fix', action='store_true',
                    help='Warn configuration issues but don\'t try to fix them')
parser.add_argument('--verbose', '-v', action='count')

args = parser.parse_args()

#
# Utility functions
#
def ddbg(s):
    global args
    if args.verbose and args.verbose >= 2:
        print(f'DBG: {s}', flush=True)

def dbg(s):
    global args
    if args.verbose:
        print(f'DBG: {s}', flush=True)

def log(s):
    print(s, flush=True)

def warn(s):
    print(f'WARN: {s}', file=sys.stderr, flush=True)

def err(s):
    print(f'ERR: {s}', file=sys.stderr, flush=True)
    sys.exit(1)

def parse_size(s):
    units = { 'K': 1 << 10, 'M': 1 << 20, 'G': 1 << 30, 'T': 1 << 40 }
    split = re.sub(r'([kKmMgGtT])', r' \1 ', s).split()
    size = 0
    for i in range(0, len(split), 2):
        try:
            num = float(split[i])
        except:
            num = float('nan')
        if not math.isfinite(num) or (i + 1 < len(split) and split[i + 1] not in units):
            raise Exception(f'invalid size "{s}"')
        if i + 1 < len(split):
            size += num * units[split[i + 1]]
        else:
            size += num
    return int(size)

# "1.5G"   - 1.5 gigabytes, returns 1610612736 (bytes)
# "1G128M" - 1 gigabyte and 128 megabytes, returns 1207959552 (bytes)
# "35.7%"  - 35.7% of whole
def parse_size_or_pct(s, whole):
    s = str(s).strip()
    if s.endswith('%'):
        return int(whole * float(s[:-1]) / 100)
    else:
        return parse_size(s)

def int_or_max(v, max_val):
    if v == 'max':
        return max_val
    return int(v)

def read_lines(path):
    with open(path, 'r', encoding='utf-8') as f:
        lines = f.read().strip().split('\n')
        if len(lines) == 1 and not len(lines[0]):
            return []
        return lines

def read_first_line(path):
    return read_lines(path)[0]

def read_cpu_idle():
        toks = read_first_line('/proc/stat').split()[1:]
        idle = int(toks[3]) + int(toks[4])
        total = 0
        for tok in toks:
            total += int(tok)
        return idle, total

def read_meminfo():
    mem_total = None
    swap_total = None
    swap_free = None
    hugetlb = None

    with open('/proc/meminfo', 'r', encoding='utf-8') as f:
        for line in f:
            toks = line.split()
            if toks[0] == 'MemTotal:':
                mem_total = int(toks[1]) * 1024
            elif toks[0] == 'SwapTotal:':
                swap_total = int(toks[1]) * 1024
            elif toks[0] == 'SwapFree:':
                swap_free = int(toks[1]) * 1024
            elif toks[0] == 'Hugetlb:':
                hugetlb = int(toks[1]) * 1024

    return mem_total, hugetlb, swap_total, swap_free

def read_memswap(cgrp_dir):
    mem_total, hugetlb, swap_total, swap_free = read_meminfo()
    swap_max = int_or_max(read_first_line(f'{cgrp_dir}/memory.swap.max'),
                          swap_total)
    swap_cur = int(read_first_line(f'{cgrp_dir}/memory.swap.current'))
    swap_avail = min(swap_total, swap_max)
    swap_free = max(min(swap_avail - swap_cur, swap_free), 0)

    return mem_total, hugetlb, swap_avail, swap_free

def read_cgroup_keyed(path):
    content = {}
    for line in read_lines(path):
        toks = line.split()
        key = toks[0]
        content[key] = toks[1]
    return content

def read_cgroup_nested_keyed(path):
    content = {}
    for line in read_lines(path):
        toks = line.split()
        key = toks[0]
        content[key] = {}
        for tok in toks[1:]:
            nkey, val = tok.split('=')
            content[key][nkey] = val
    return content

def dump_json(data, path):
    dirname, basename = os.path.split(path)
    tf = open(os.path.join(dirname, 'P{}-{}.tmp'.format(os.getpid(), basename)), 'w')
    tf.write(json.dumps(data, sort_keys=True, indent=4))
    tf.close()
    os.rename(tf.name, path)

def svc_to_jobid(svc):
    global args

    assert svc.startswith(args.svc_prefix) and svc.endswith(SVC_SUFFIX)
    return svc[len(args.svc_prefix):-len(SVC_SUFFIX)]

def time_interval(at, now):
    if at is None:
        return 0
    else:
        intv = max(int(now - at), 1)
        return intv

#
# Classes
#
class Config:
    def __init__(self, cfg):
        mem_total, hugetlb, swap_total, swap_free = read_meminfo()

        self.main_slice = cfg['main_slice']
        self.host_slice = cfg['host_slice']
        self.side_slice = cfg['side_slice']
        self.main_cpu_weight = int(cfg['main_cpu_weight'])
        self.host_cpu_weight = int(cfg['host_cpu_weight'])
        self.side_cpu_weight = int(cfg['side_cpu_weight'])
        self.main_io_weight = int(cfg['main_io_weight'])
        self.host_io_weight = int(cfg['host_io_weight'])
        self.side_io_weight = int(cfg['side_io_weight'])
        self.side_memory_high = parse_size_or_pct(cfg['side_memory_high'], mem_total)
        self.side_swap_max = parse_size_or_pct(cfg['side_swap_max'], swap_total)
        self.cpu_headroom_period = float(cfg['cpu_headroom_period'])
        self.cpu_headroom = float(cfg['cpu_headroom'])
        self.cpu_min_avail = float(cfg['cpu_min_avail'])
        self.cpu_floor = float(cfg['cpu_floor'])
        self.cpu_throttle_period = float(cfg['cpu_throttle_period'])

        self.ov_cpu_duration = float(cfg['overload_cpu_duration'])
        self.ov_memp_thr = float(cfg['overload_mempressure_threshold'])
        self.ov_hold = float(cfg['overload_hold'])
        self.ov_hold_max = float(cfg['overload_hold_max'])
        self.ov_hold_decay = float(cfg['overload_hold_decay_rate'])

        self.crit_swapfree_thr = \
            parse_size_or_pct(cfg['critical_swapfree_threshold'],
                              min(self.side_swap_max, swap_total))
        self.crit_memp_thr = float(cfg['critical_mempressure_threshold'])
        self.crit_iop_thr = float(cfg['critical_iopressure_threshold'])

        if 'scribe_category' in cfg:
            self.scribe_category = cfg['scribe_category']
            self.scribe_interval = float(cfg['scribe_interval'])
        else:
            self.scribe_category = None
            self.scribe_interval = 1

class JobFile:
    def __init__(self, ino, path, fh):
        self.ino = ino
        self.path = path
        self.fh = fh

    def __repr__(self):
        return f'{self.ino}:{self.path}'

class Job:
    def __init__(self, cfg, jobfile):
        global args

        jobid = cfg['id']
        if not re.search('^[A-Za-z0-9-_.]*$', jobid):
            raise Exception(f'"{jobid}" is not a valid identifier')

        frozen_exp = None
        if 'frozen_expiration' in cfg:
            frozen_exp = float(cfg['frozen_expiration'])

        self.jobfile = jobfile
        self.jobid = jobid
        self.args = cfg['args']
        self.envs = cfg['envs']
        self.frozen_exp = frozen_exp
        self.frozen_at = None
        self.done = False
        self.kill_why = None
        self.killed = False
        self.svc_name = f'{args.svc_prefix}{jobid}{SVC_SUFFIX}'
        self.svc_status = None
        self.working_dir = cfg['working_dir'] if 'working_dir' in cfg else None;

    def update_frozen(self, freeze, now):
        changed = False
        if not self.frozen_at and freeze:
            self.frozen_at = now
            changed = True
        elif self.frozen_at and not freeze:
            self.frozen_at = None
            changed = True

        path = pathlib.Path(f'{CGRP_BASE}/{config.side_slice}/{self.svc_name}/cgroup.freeze')
        if not path.exists():
            if changed:
                warn(f'Failed to freeze {self.jobid}')
            return

        if int(freeze) == int(read_first_line(path)):
            return

        with path.open('w') as f:
            f.write(str(int(freeze)))

    def maybe_kill(self):
        if not self.kill_why:
            return

        path = pathlib.Path(f'{CGRP_BASE}/{config.side_slice}/{self.svc_name}/cgroup.procs')
        if not path.exists():
            return

        pids = read_lines(path)
        if len(pids):
            dbg(f'killing {self.jobid}: {pids}')
            for pid in pids:
                os.kill(int(pid), signal.SIGKILL)
            log(f'JOB: Attempted to kill {self.jobid} ({len(pids)} processes)')

    def kill(self, why):
        dbg(f'kill requested for {self.jobid} why="{why}" cur_why="{self.kill_why}"')
        if not self.kill_why:
            self.kill_why = why
        self.maybe_kill()

    def refresh_status(self, now):
        self.svc_status = "<UNKNOWN>"

        out = subprocess.run(['systemctl', 'status', self.svc_name],
                             stdout=subprocess.PIPE,
                             stderr=subprocess.DEVNULL).stdout.decode('utf-8')
        for line in out.split('\n'):
            toks = line.split(maxsplit=1)
            if len(toks) == 2 and toks[0] == 'Active:':
                self.svc_status = toks[1]
                break

        if '(exited)' in self.svc_status:
            self.done = True

        if 'failed' in self.svc_status:
            self.done = True
            self.killed = True

class SysInfo:
    def __init__(self, pressure_dir, nr_hist_intvs):
        self.pressure_dir = pressure_dir
        self.cpu_total_hist = [None] * (nr_hist_intvs + 1)
        self.cpu_idle_hist = [None] * (nr_hist_intvs + 1)
        self.cpu_side_hist = [None] * (nr_hist_intvs + 1)
        self.cpu_hist_idx = 0
        self.memp_1min = 0
        self.memp_5min = 0
        self.iop_1min = 0
        self.iop_5min = 0
        self.mem_total = 0
        self.hugetlb = 0
        self.swap_avail = 0
        self.swap_free = 0
        self.swap_free_pct = 100
        self.critical = False
        self.critical_why = None
        self.overload = False
        self.overload_why = None

    def update(self):
        global config

        side_cgrp = f'{CGRP_BASE}/{config.side_slice}'

        # cpu stats
        cpu_idle, cpu_total = read_cpu_idle()
        cpu_total = cpu_total / USER_HZ * 1_000_000
        cpu_idle = cpu_idle / USER_HZ * 1_000_000
        cpu_stat = read_cgroup_keyed(f'{side_cgrp}/cpu.stat')
        cpu_side = float(cpu_stat['usage_usec'])

        next_idx = (self.cpu_hist_idx + 1) % len(self.cpu_idle_hist)
        self.cpu_total_hist[next_idx] = cpu_total
        self.cpu_idle_hist[next_idx] = cpu_idle
        self.cpu_side_hist[next_idx] = cpu_side
        self.cpu_hist_idx = next_idx

        # memory and io pressures
        pres = read_cgroup_nested_keyed(self.pressure_dir + '/memory.pressure')
        self.memp_1min = float(pres['full']['avg60'])
        self.memp_5min = float(pres['full']['avg300'])

        pres = read_cgroup_nested_keyed(self.pressure_dir + '/io.pressure')
        self.iop_1min = float(pres['full']['avg60'])
        self.iop_5min = float(pres['full']['avg300'])

        # swap
        (self.mem_total, self.hugetlb,
         self.swap_avail, self.swap_free) = read_memswap(side_cgrp)
        self.swap_free_pct = 100
        if self.swap_avail:
            self.swap_free_pct = self.swap_free / self.swap_avail * 100

    def __cpu_lridx(self, nr_intvs):
        assert nr_intvs > 0 and nr_intvs < len(self.cpu_idle_hist)
        ridx = self.cpu_hist_idx
        lidx = (ridx - nr_intvs) % len(self.cpu_idle_hist)
        if self.cpu_total_hist[lidx] is not None:
            return lidx, ridx
        else:
            return None, None

    def __cpu_avg(self, hist, nr_intvs):
        lidx, ridx = self.__cpu_lridx(nr_intvs)
        if lidx is None:
            return 0
        total = self.cpu_total_hist[ridx] - self.cpu_total_hist[lidx]
        delta = hist[ridx] - hist[lidx]
        return min(max(delta / total * 100, 0), 100)

    def __cpu_min_max(self, hist, nr_intvs):
        pct_min = 100
        pct_max = 0
        idx, ridx = self.__cpu_lridx(nr_intvs)
        if idx is None:
            return 0, 0

        while idx != ridx:
            nidx = (idx + 1) % len(self.cpu_idle_hist)
            total = self.cpu_total_hist[nidx] - self.cpu_total_hist[idx]
            delta = hist[nidx] - hist[idx]
            pct = min(max(delta / total * 100, 0), 100)
            pct_min = min(pct_min, pct)
            pct_max = max(pct_max, pct)
            idx = nidx

        return pct_min, pct_max

    def cpu_avg_idle(self, nr_intvs):
        return self.__cpu_avg(self.cpu_idle_hist, nr_intvs)

    def cpu_min_max_idle(self, nr_intvs):
        return self.__cpu_min_max(self.cpu_idle_hist, nr_intvs)

    def cpu_avg_side(self, nr_intvs):
        return self.__cpu_avg(self.cpu_side_hist, nr_intvs)

    def cpu_min_max_side(self, nr_intvs):
        return self.__cpu_min_max(self.cpu_side_hist, nr_intvs)

class SysChecker:
    def __init__(self):
        global args, config

        self.fix = not args.dont_fix
        self.main_cgrp = f'{CGRP_BASE}/{config.main_slice}'
        self.host_cgrp = f'{CGRP_BASE}/{config.host_slice}'
        self.side_cgrp = f'{CGRP_BASE}/{config.side_slice}'

        self.active = False
        self.last_check_at = 0
        self.last_warns = []
        self.warns = []
        self.fixed = False

        self.root_dev = None
        self.root_devnr = None
        self.mem_total = 0
        self.swap_avail = 0
        self.swap_free = 0
        self.swappiness = 0
        self.hugetlb = 0

        # find the root device maj/min
        if args.dev is None:
            root_part = None
            for line in read_lines('/proc/mounts'):
                toks = line.split()
                if toks[1] == '/':
                    if toks[0].startswith('/dev/'):
                        root_part = toks[0][len('/dev/'):]
                    break
            if root_part is None:
                warn('SYSCFG: failed to find root mount')
                return

            if root_part.startswith('sd'):
                self.root_dev = re.sub(r'^(sd[^0-9]*)[0-9]*$', r'\1', root_part)
            elif root_part.startswith('nvme'):
                self.root_dev = re.sub(r'^(nvme[^p]*)(p[0-9])?$', r'\1', root_part)
            else:
                raise Exception(f'unknown device {root_part}')
        else:
            self.root_dev = args.dev

        try:
            out = subprocess.run(['stat', '-c', '0x%t 0x%T', f'/dev/{self.root_dev}'],
                                 stdout=subprocess.PIPE).stdout.decode('utf-8')
            toks = out.split()
            self.root_devnr = f'{int(toks[0], 0)}:{int(toks[1], 0)}'
        except Exception as e:
            warn(f'SYSCFG: failed to find root device ({e})')

    def __check_and_fix_rootfs(self):
        toks = None
        for line in read_lines('/proc/mounts'):
            toks = line.split()
            if toks[1] == '/':
                break

        if toks is None or toks[1] != '/':
            return ['failed to find root fs mount entry']

        if toks[2] != 'btrfs':
            return ['root filesystem is not btrfs']

        if 'discard=async' in toks[3]:
            return []

        fixed = ''
        if self.fix:
            try:
                subprocess.check_call(['mount', '-o', 'remount,discard=async', '/'])
            except Exception as e:
                return [f'failed to enable async discard on root fs ({e})']

            self.fixed = True
            fixed = ', enabled'

        return [f'async discard disabled on root fs{fixed}']

    def __check_memswap(self):
        global config

        warns = []

        (self.mem_total, self.hugetlb,
         self.swap_avail, self.swap_free) = read_memswap(self.side_cgrp)

        if self.swap_avail < 0.9 * (self.mem_total / 4):
            warns.append(f'available swap ({self.swap_avail/(1<<30):.2f}G) '
                         f'is smaller than 1/4 of physical memory')

        if self.swap_avail < 0.9 * config.side_swap_max:
            warns.append(f'available swap ({self.swap_avail/(1<<30):.2f}G) '
                         f'is smaller than side-swap-max')

        self.swappiness = int(read_first_line('/proc/sys/vm/swappiness'))
        if self.swappiness < 60:
            warns.append('swappiness ({self.swappiness}) is lower than default 60')

        return warns

    def __check_freezer(self):
        global config

        if not os.path.exists(f'{self.side_cgrp}/cgroup.freeze'):
            return ['freezer is not available']
        return []

    def __check_and_fix_io_latency_off(self):
        warns = []
        root_path = pathlib.Path(CGRP_BASE)
        for path in root_path.glob('**/io.latency'):
            try:
                latcfg = read_cgroup_nested_keyed(path)
                if self.root_devnr not in latcfg:
                    continue
                fixed = ''
                if self.fix:
                    with path.open('w') as f:
                        f.write(f'{self.root_devnr} target=0')
                    self.fixed = True
                    fixed = ', disabled'
                warns.append(f'{str(path)} has non-null config{fixed}')
            except Exception as e:
                warns.append(f'failed to check and disable {str(path)} ({e})')
        return warns

    def __check_and_fix_main_memory_low(self):
        global config
        warns = []

        main_memory_low = None
        try:
            main_path = pathlib.Path(self.main_cgrp)
            for subdir in ('', 'workload-tw.slice/', 'workload-tw.slice/*.task/',
                           'workload-tw.slice/*.task/task/'):
                for path in main_path.glob(f'{subdir}memory.low'):
                    low = int_or_max(read_first_line(path), self.mem_total)
                    if low < (self.mem_total - self.hugetlb) / 3:
                        if main_memory_low:
                            fixed = ''
                            if self.fix:
                                try:
                                    with path.open('w') as f:
                                        f.write(f'{main_memory_low}')
                                except Exception as e:
                                    warns.append(f'Failed to set {str(path)} to {main_memory_low} ({e})')
                                else:
                                    self.fixed = True
                                    fixed = f', configured to {main_memory_low}'
                            warns.append(f'{str(path)} is lower than a third of system '
                                         f'memory{fixed}')
                        else:
                            warns.append(f'{str(path)} is lower than a third of system '
                                         f'memory, no idea what to config')
                    else:
                        main_memory_low = low
        except Exception as e:
            warns.append(f'failed to check {config.main_slice}/* memory.low ({e})')

        return warns

    def __check_and_fix_side_memory_high(self):
        global config
        warns = []
        need_fix = False
        try:
            high = read_first_line(f'{self.side_cgrp}/memory.high')
            if high == "max":
                high = self.mem_total
            else:
                high = int(high)

            if high >> 20 != config.side_memory_high >> 20:
                warns.append(f'{config.side_slice} memory.high is not {config.side_memory_high}')
                need_fix = True
        except Exception as e:
            warns.append(f'failed to check {config.side_slice} memory.high ({e})')
            need_fix = True

        if self.fix and need_fix:
            try:
                subprocess.check_call(['systemctl', 'set-property', config.side_slice,
                                       f'MemoryHigh={config.side_memory_high}',
                                       f'MemorySwapMax={config.side_swap_max}'])
                with open(f'{self.side_cgrp}/memory.high', 'w') as f:
                    f.write(f'{config.side_memory_high}')
            except Exception as e:
                warns.append(f'Failed to set {config.side_slice} resource configs ({e})')
            else:
                self.fixed = True
        return warns

    def __check_weight(self, slice, knob, weight, prefix=None):
        try:
            line = read_first_line(f'{CGRP_BASE}/{slice}/{knob}')
            if prefix:
                line = line.split()[1]
            weight = int(line)
        except Exception as e:
            return[f'failed to check {slice}/{knob} ({e})']

        if weight == weight:
            return []
        else:
            return [f'{slice}/{knob} != {weight}']

    def __update_weight(self, slice, knob, weight, systemd_key=None, prefix=None):
        try:
            with open(f'{CGRP_BASE}/{slice}/{knob}', 'w') as f:
                if prefix:
                    f.write(f'{prefix} {weight}')
                else:
                    f.write(f'{weight}')
        except Exception as e:
            return[f'Failed to set {slice}/{knob} to {weight} ({e})']

        if systemd_key:
            try:
                subprocess.check_call(['systemctl', 'set-property', slice,
                                       f'{systemd_key}={weight}'])
            except Exception as e:
                return [f'Failed to set {slice} {systemd_key} to {weight} ({e})']

        return []

    def __check_cpu_weights(self):
        global config

        if 'cpu' not in read_first_line(f'{CGRP_BASE}/cgroup.subtree_control'):
            return['cpu controller not enabled at root']

        warns = []
        warns += self.__check_weight(config.main_slice, 'cpu.weight',
                                     config.main_cpu_weight)
        warns += self.__check_weight(config.host_slice, 'cpu.weight',
                                     config.host_cpu_weight)
        warns += self.__check_weight(config.side_slice, 'cpu.weight',
                                     config.side_cpu_weight)
        return warns

    def __fix_cpu_weights(self):
        global config

        try:
            with open(f'{CGRP_BASE}/cgroup.subtree_control', 'w') as f:
                f.write('+cpu')
        except Exception as e:
            return [f'failed to enable CPU controller in the root cgroup ({e})']

        warns = []
        warns += self.__update_weight(config.main_slice, 'cpu.weight',
                                      config.main_cpu_weight, 'CPUWeight')
        warns += self.__update_weight(config.host_slice, 'cpu.weight',
                                      config.host_cpu_weight, 'CPUWeight')
        warns += self.__update_weight(config.side_slice, 'cpu.weight',
                                      config.side_cpu_weight, 'CPUWeight')
        self.fixed |= len(warns) == 0
        return warns

    def __check_io_weights(self):
        global config

        if self.root_devnr is None:
            return [f'failed to find devnr for {root_part}']

        try:
            enabled = False
            for line in read_lines(f'{CGRP_BASE}/io.cost.qos'):
                toks = line.split()
                if self.root_devnr == toks[0]:
                    for t in toks:
                        if t == 'enable=1':
                            enabled = True
                            break
                    break
        except Exception as e:
            return [f'failed to verify iocost for {self.root_dev} ({e})']

        if not enabled:
            return [f'iocost not enabled on {self.root_dev}']

        warns = []
        warns += self.__check_weight(config.main_slice, 'io.weight',
                                     config.main_io_weight, 'default')
        warns += self.__check_weight(config.host_slice, 'io.weight',
                                     config.host_io_weight, 'default')
        warns += self.__check_weight(config.side_slice, 'io.weight',
                                     config.side_io_weight, 'default')
        return warns

    def __fix_io_weights(self):
        try:
            with open(f'{CGRP_BASE}/io.cost.qos', 'w') as f:
                f.write(f'{self.root_devnr} enable=1')
        except Exception as e:
            return [f'failed to enable iocost for {self.root_dev} ({e})']

        warns = []
        warns += self.__update_weight(config.main_slice, 'io.weight',
                                      config.main_io_weight, 'IOWeight', 'default')
        warns += self.__update_weight(config.host_slice, 'io.weight',
                                      config.host_io_weight, 'IOWeight', 'default')
        warns += self.__update_weight(config.side_slice, 'io.weight',
                                      config.side_io_weight, 'IOWeight', 'default')
        self.fixed |= len(warns) == 0
        return warns

    def __check(self, now):
        global config

        self.last_check_at = now
        self.last_warns = self.warns
        self.warns = []

        # run the checks and fixes
        self.warns += self.__check_and_fix_rootfs()
        self.warns += self.__check_memswap()
        self.warns += self.__check_freezer()
        self.warns += self.__check_and_fix_io_latency_off()
        self.warns += self.__check_and_fix_main_memory_low()
        self.warns += self.__check_and_fix_side_memory_high()

        warns = self.__check_cpu_weights()
        # Enabling CPU controller carries significant overhead.  Fix
        # it iff there are active side jobs.
        if self.fix and len(warns) and self.active:
            warns.append('Fixing cpu.weights')
            warns += self.__fix_cpu_weights()
        self.warns += warns

        warns = self.__check_io_weights()
        if self.fix and len(warns):
            warns.append('Fixing io.weights')
            warns += self.__fix_io_weights()
        self.warns += warns

        # log if changed
        if self.warns != self.last_warns:
            if len(self.warns):
                i = 0
                for w in self.warns:
                    warn(f'SYSCFG[{i}]: {w}')
                    i += 1
            else:
                log(f'SYSCFG: all good')

    def check(self, now):
        self.fixed = False
        self.__check(now)
        if self.fixed:
            self.__check(now)

    def periodic_check(self, intv, now):
        if now - self.last_check_at >= intv:
            self.check(now)

    def update_active(self, active):
        global args

        active = bool(active)
        if self.active == active:
            return

        if not args.dont_fix:
            if active:
                log('SYSCFG: overriding root slice DisableControllers')
                try:
                    with open(systemd_root_override_file, 'w') as f:
                        f.write('[Slice]\n'
                                'DisableControllers=\n')
                    subprocess.check_call(['systemctl', 'daemon-reload'])
                except Exception as e:
                    warn(f'SYSCFG: failed to overried root slice DisableControllers ({e})')
            else:
                log('SYSCFG: reverting root slice DisableControllers')
                try:
                    os.remove(systemd_root_override_file)
                    subprocess.check_call(['systemctl', 'daemon-reload'])
                except Exception as e:
                    warn(f'SYSCFG: failed to revert root slice DisableControllers ({e})')

        self.active = active

class Scriber:
    def __init__(self, cat, intv):
        self.category = cat
        self.interval = intv
        self.last_at = 0
        self.scribe_proc = None
        self.disabled = False

    def should_log(self, now):
        if int(now) - int(self.last_at) < self.interval:
            return False

        if self.scribe_proc and self.scribe_proc.poll() is None:
            return False

        return True

    def log(self, msg, now):
        if self.disabled:
            return
        self.last_at = now
        if self.scribe_proc:
            self.scribe_proc.wait()
        try:
            self.scribe_proc = subprocess.Popen(['scribe_cat', self.category, msg])
        except Exception as e:
            warn(f'Failed to run scribe_cat ({e}), disabling scribe logging')
            self.disabled = True

#
# Implementation
#
def list_side_services():
    global config, args

    out = subprocess.run(['systemctl', 'list-units', '-l', args.svc_prefix + '*'],
                         stdout=subprocess.PIPE).stdout.decode('utf-8')
    svcs = []
    for line in out.split('\n'):
        toks = line[2:].split();
        if len(toks) and \
           toks[0].startswith(args.svc_prefix) and toks[0].endswith(SVC_SUFFIX):
            svcs.append(toks[0])
    return svcs

def process_job_dir(jobfiles, jobs, now):
    global args

    job_dir_path = pathlib.Path(args.jobdir)
    input_jobfiles = {}

    # Open all job files.
    for path in job_dir_path.glob('*'):
        try:
            if path.is_symlink() or not path.is_file():
                raise Exception('Invalid file type')
            fh = path.open('r', encoding='utf-8')
            ino = os.fstat(fh.fileno()).st_ino
            input_jobfiles[ino] = JobFile(ino, str(path), fh)
        except Exception as e:
            warn(f'Failed to open {path} ({e})')

    # Let's find out which files are gone and which are new.
    gone_jobfiles = []
    new_jobfiles = []

    for ino, jf in jobfiles.items():
        if jf.ino not in input_jobfiles:
            gone_jobfiles.append(jf)
    for ino, jf in input_jobfiles.items():
        if jf.ino not in jobfiles:
            new_jobfiles.append(jf)

    if len(gone_jobfiles):
        ddbg(f'gone_jobfiles: {[jf.path for jf in gone_jobfiles]}')
    if len(new_jobfiles):
        ddbg(f'new_jobfiles: {[jf.path for jf in new_jobfiles]}')

    for jf in gone_jobfiles:
        del jobfiles[jf.ino]

    # Collect active jobids and determine jobs to kill.
    jobids = set()
    jobs_to_kill = {}

    for i, job in jobs.items():
        if job.jobfile.ino in jobfiles:
            jobids.add(job.jobid)
        else:
            jobs_to_kill[job.jobid] = job

    if len(jobs_to_kill):
        ddbg(f'jobs_to_kill: {[jobid for jobid in jobs_to_kill]}')

    # Load new job files
    jobs_to_start = {}

    for jf in new_jobfiles:
        jf_jobids = set()
        jf_jobs = {}
        try:
            parsed = json.load(jf.fh)
            for ent in parsed['sideloader_jobs']:
                job = Job(ent, jf)
                if job.jobid in jobids or job.jobid in jf_jobids:
                    raise Exception(f'Duplicate job id {job.jobid}')
                jf_jobids.add(job.jobid)
                jf_jobs[job.jobid] = job
        except Exception as e:
            warn(f'Failed to load {path} ({e})')
        else:
            jobfiles[jf.ino] = jf
            jobids = jobids.union(jf_jobids)
            jobs_to_start.update(jf_jobs)

    if len(jobs_to_start):
        ddbg(f'jobs_to_start: {[jobid for jobid in jobs_to_start]}')

    return jobs_to_kill, jobs_to_start

def count_active_jobs(jobs):
    count = 0
    for jobid, job in jobs.items():
        if job.frozen_at is None and not job.done:
            count += 1
    return count

def count_frozen_jobs(jobs):
    count = 0
    for jobid, job in jobs.items():
        if job.frozen_at is not None:
            count += 1
    return count

def config_cpu_max(pct):
    global config

    cpu_max_file = f'{CGRP_BASE}/{config.side_slice}/cpu.max'
    period = int(config.cpu_throttle_period * 1_000_000)
    quota = int(multiprocessing.cpu_count() * period * pct / 100)

    try:
        cur_quota, cur_period = read_first_line(cpu_max_file).split()
        cur_quota = 100 if cur_quota == 'max' else int(cur_quota)
        cur_period = int(cur_period)
        if period == cur_period and quota == cur_quota:
            return

        with open(cpu_max_file, 'w') as f:
            f.write(f'{quota} {period}')
    except Exception as e:
        warn(f'Failed to configure {cpu_max_file} ({e})')

# Run
config_mod_at = os.path.getmtime(args.config)
config = Config(json.load(open(args.config, 'r'))['sideloader_config'])
dbg(f'Config: {config.__dict__}')
log(f'INIT: sideloads in {config.side_slice}, main workloads in {config.main_slice}')

nr_active = 0
critical_at = None
critical_why = None
overload_at = None
overload_hold_from = 0
overload_hold = 0
overload_why = None
jobfiles = {}
jobs_pending = {}
jobs = {}
now = time.time()

nr_cpu_headroom_intvs = math.ceil(config.cpu_headroom_period / interval)
nr_cpu_overload_intvs = math.ceil(config.ov_cpu_duration / interval)
nr_cpu_hist_intvs = max(nr_cpu_headroom_intvs, nr_cpu_overload_intvs)
sysinfo = SysInfo(f'{CGRP_BASE}/{config.side_slice}', nr_cpu_hist_intvs)
syschecker = SysChecker()

if config.scribe_category:
    scriber = Scriber(config.scribe_category, config.scribe_interval)
else:
    scriber = None

# Init sideload.slice
subprocess.check_call(['systemctl', 'set-property', config.side_slice,
                       f'CPUWeight={config.side_cpu_weight}',
                       f'MemoryHigh={config.side_memory_high}',
                       f'IOWeight={config.side_io_weight}'])

# List sideload.slice and kill everything which isn't in the jobdir.
# Don't worry about matching or missing ones, the main loop will
# handle them.
svcs = list_side_services()
jobs_to_kill, jobs_to_start = process_job_dir({}, {}, now)
svcs_to_stop = []

for svc in svcs:
    if svc_to_jobid(svc) not in jobs_to_start:
        svcs_to_stop.append(svc)

if len(svcs_to_stop):
    log(f'JOB: Stopping stray services {svcs_to_stop}')
    subprocess.run(['systemctl', 'stop'] + svcs_to_stop)
    subprocess.run(['systemctl', 'reset-failed'] + svcs_to_stop)

# The main loop
while True:
    last = now
    now = time.time()

    # Apply config change, only cpu headroom can be updated while running
    if config_mod_at != os.path.getmtime(args.config):
        config_mod_at = os.path.getmtime(args.config)
        new_cfg = json.load(open(args.config, 'r'))['sideloader_config']
        new_cpu_headroom = float(new_cfg['cpu_headroom'])
        if config.cpu_headroom != new_cpu_headroom:
            log(f'CFG: CPU headroom changed from {config.cpu_headroom:.2f} to {new_cpu_headroom:.2f}')
            config.cpu_headroom = new_cpu_headroom

    # Handle job starts and stops
    jobs_to_kill, jobs_to_start = process_job_dir(jobfiles, jobs, now)

    for jobid, job in jobs_to_kill.items():
        log(f'JOB: Stopping {job.svc_name}')
        subprocess.run(['systemctl', 'stop', job.svc_name])
        subprocess.run(['systemctl', 'reset-failed', job.svc_name])
        del jobs[jobid]

    # Start new jobs iff not overloaded
    jobs_pending.update(jobs_to_start)
    if not overload_at:
        for jobid, job in jobs_pending.items():
            log(f'JOB: Starting {job.svc_name}')
            jobs[jobid] = job
            syschecker.update_active(count_active_jobs(jobs))
            cmd = ['systemd-run', '-r', '-p', 'TimeoutStopSec=5',
                   '--slice', config.side_slice, '--unit', job.svc_name]
            if job.working_dir is not None:
                cmd += ['--working-directory', job.working_dir]
            for env in job.envs:
                cmd += ['-E', env]
            cmd += job.args
            subprocess.run(cmd)
        jobs_pending = {}

    # Read the current system state
    sysinfo.update()

    # Do syscfg check every 10 secs if there are jobs; otherwise, every 60s
    syschecker.periodic_check(10 if len(jobs) > 0 else 60, now)

    cpu_cur_idle = min(sysinfo.cpu_avg_idle(nr_cpu_headroom_intvs),
                       sysinfo.cpu_avg_idle(1))
    cpu_cur_side = max(sysinfo.cpu_avg_side(nr_cpu_headroom_intvs),
                       sysinfo.cpu_avg_side(1))
    cpu_avail = max(cpu_cur_side + cpu_cur_idle - config.cpu_headroom,
                    config.cpu_floor)

    cpu_avg_idle = sysinfo.cpu_avg_idle(nr_cpu_overload_intvs)
    cpu_avg_side = sysinfo.cpu_avg_side(nr_cpu_overload_intvs)

    # Handle critical condition
    if sysinfo.swap_free <= config.crit_swapfree_thr:
        critical_why = (f'swap-left {sysinfo.swap_free>>20}MB is lower than '
                        f'critical threshold {config.crit_swapfree_thr>>20}MB')
    elif sysinfo.memp_5min >= config.crit_memp_thr:
        critical_why = (f'5min memory pressure {sysinfo.memp_5min:.2f} is higher than '
                        f'critical threshold {config.crit_memp_thr:.2f}')
    elif sysinfo.iop_5min >= config.crit_iop_thr:
        critical_why = (f'5min io pressure {sysinfo.iop_5min:.2f} is higher than '
                        f'critical threshold {config.crit_iop_thr:.2f}')
    else:
        critical_why = None

    if critical_why is not None:
        if critical_at is None:
            log(f'CRITICAL: {critical_why}')
            crtical_at = now
        if overload_at is None:
            overload_at = now
        overload_why = 'resource critical'
        overload_hold = config.ov_hold_max
        for jobid, job in jobs.items():
            job.kill(f'resource critical {critical_why}')
    elif critical_at is not None:
        log(f'CRITICAL: end, resuming normal operation')
        critical_at = None

    # Handle overload condition
    side_margin = max(cpu_avg_side + cpu_avg_idle - config.cpu_headroom, 0)
    if side_margin < config.cpu_min_avail:
        overload_why = (f'cpu margin {side_margin:.2f} is too low')
    elif sysinfo.memp_1min >= config.ov_memp_thr:
        overload_why = (f'1min memory pressure {sysinfo.memp_1min:.2f} is over '
                        f'the threshold {config.ov_memp_thr:.2f}')
    else:
        overload_why = None

    if overload_why is not None:
        # Log if we're just getting overloaded
        if not overload_at:
            log(f'OVERLOAD: {overload_why}, hold={int(overload_hold)}s')
            overload_at = now
            overload_hold = min(config.ov_hold + overload_hold, config.ov_hold_max)
        overload_hold_from = now
    elif overload_at is not None:
        if now > overload_hold_from + overload_hold:
            log('OVERLOAD: end, resuming normal operation')
            overload_at = None

    if overload_at:
        for jobid, job in jobs.items():
            job.update_frozen(True, now)
            ddbg(f'{jobid} frozen for {int(now - job.frozen_at)}s exp={job.frozen_exp}')
            if now - job.frozen_at >= job.frozen_exp:
                job.kill('frozen for too long')
    else:
        overload_hold = max(overload_hold - config.ov_hold_decay, 0)
        for jobid, job in jobs.items():
            job.update_frozen(False, now)

    # Process frozen timeouts
    for jobid, job in jobs.items():
        job.maybe_kill()

    # Configure side's cpu.max and update active state
    nr_active = count_active_jobs(jobs)
    if nr_active > 0:
        config_cpu_max(cpu_avail)

    syschecker.update_active(nr_active)

    # Refresh service status and report
    for jobid, job in jobs.items():
        job.refresh_status(now)

    status = {
        'sideloader_status': {
            'now': str(datetime.datetime.fromtimestamp(now)),
            'sysconfig_warnings_at': str(datetime.datetime.fromtimestamp(syschecker.last_check_at)),
            'sysconfig_warnings': syschecker.warns,
            'jobs': [ {
                'id': jobid,
                'path' : job.jobfile.path,
                'service_name': job.svc_name,
                'service_status': job.svc_status,
                'frozen_for': time_interval(job.frozen_at, now),
                'is_killed': int(job.killed),
                'is_done': int(job.done),
                'kill_why': f'{job.kill_why if job.kill_why else ""}',
            } for jobid, job in jobs.items() ],
            'jobs_pending': [ {
                'id': jobid,
                'path': job.jobfile.path,
            } for jobid, job in jobs_pending.items() ],
            'sysinfo': {
                'cpu_cur_idle': cpu_cur_idle,
                'cpu_cur_side': cpu_cur_side,
                'cpu_avg_idle': cpu_avg_idle,
                'cpu_avg_side': cpu_avg_side,
                'cpu_avail': cpu_avail,
                'mempressure_1min': sysinfo.memp_1min,
                'mempressure_5min': sysinfo.memp_5min,
                'iopressure_1min': sysinfo.iop_1min,
                'iopressure_5min': sysinfo.iop_5min,
                'swap_avail_gb': sysinfo.swap_avail/(1<<30),
                'swap_free_pct': sysinfo.swap_free_pct,
            },
            'overload': {
                'critical_for': time_interval(critical_at, now),
                'overload_for': time_interval(overload_at, now),
                'overload_hold': max(overload_hold_from + overload_hold - now, 0),
                'critical_why': f'{critical_why if critical_why else ""}',
                'overload_why': f'{overload_why if overload_why else ""}',
            }
        }
    }
    dump_json(status, args.status)

    if scriber and scriber.should_log(now):
        sstatus = {
            'int': {
                'time': int(now),
                'critical': critical_at is not None,
                'overload': overload_at is not None,
                'nr-jobs': len(jobs),
                'nr-active-jobs': nr_active,
                'nr-frozen-jobs': count_frozen_jobs(jobs),
                'nr-pending-jobs': len(jobs_pending),
            },
            'float': {
                'cpu-cur-idle': cpu_cur_idle,
                'cpu-cur-side': cpu_cur_side,
                'cpu-avg-idle': cpu_avg_idle,
                'cpu-avg-side': cpu_avg_side,
                'cpu-avail': cpu_avail,
                'mempressure-1min': sysinfo.memp_1min,
                'mempressure-5min': sysinfo.memp_5min,
                'iopressure-1min': sysinfo.iop_1min,
                'iopressure-5min': sysinfo.iop_5min,
                'swap-free-pct': sysinfo.swap_free_pct,
            },
            'normal': {
                'hostname': platform.node(),
            },
        }
        dump_json(sstatus, args.scribe)
        scriber.log(json.dumps(sstatus), now)

    time.sleep(interval)

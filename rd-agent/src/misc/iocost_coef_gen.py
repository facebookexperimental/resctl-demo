#!/usr/bin/env python3
#
# Copyright (C) 2019 Tejun Heo <tj@kernel.org>
# Copyright (C) 2019 Andy Newell <newella@fb.com>
# Copyright (C) 2019 Facebook

desc = """
Generate linear IO cost model coefficients used by the blk-iocost
controller.  If the target raw testdev is specified, destructive tests
are performed against the whole device; otherwise, on
./iocost-coef-fio.testfile.  The result can be written directly to
/sys/fs/cgroup/io.cost.model.

On high performance devices, --numjobs > 1 is needed to achieve
saturation.

--json output includes suggested parameters for
/sys/fs/cgroup/io.cost.qos.  While the parameters can be useful,
they're very rough heuristics.

See Documentation/admin-guide/cgroup-v2.rst and block/blk-iocost.c
for more details.
"""

import argparse
import re
import json
import glob
import os
import sys
import time
import signal
import atexit
import random
import shutil
import tempfile
import subprocess

parser = argparse.ArgumentParser(description=desc,
                                 formatter_class=argparse.RawTextHelpFormatter)
parser.add_argument('--testdev', metavar='DEV',
                    help='Raw block device to use for testing, ignores --testfile-size-gb')
parser.add_argument('--testfile-dev', metavar='DEV',
                    help='Override testfile device detection')
parser.add_argument('--testfile-size-gb', type=float, metavar='GIGABYTES', default=16,
                    help='Testfile size in gigabytes (default: %(default)s)')
parser.add_argument('--duration', type=int, metavar='SECONDS', default=120,
                    help='Individual test run duration in seconds (default: %(default)s)')
parser.add_argument('--seqio-block-mb', metavar='MEGABYTES', type=int, default=128,
                    help='Sequential test block size in megabytes (default: %(default)s)')
parser.add_argument('--seq-depth', type=int, metavar='DEPTH', default=64,
                    help='Sequential test queue depth (default: %(default)s)')
parser.add_argument('--rand-depth', type=int, metavar='DEPTH', default=64,
                    help='Random test queue depth (default: %(default)s)')
parser.add_argument('--numjobs', type=int, metavar='JOBS', default=1,
                    help='Number of parallel fio jobs to run (default: %(default)s)')
parser.add_argument('--json', metavar='FILE',
                    help='Store the results to the specified json file')
parser.add_argument('--model-override', metavar='"rbps=XXX rseqiops=XXX..."',
                    help='Use the specified model params instead of testing for them')
parser.add_argument('--qos-override', metavar='"rpct=XXX rlat=XXX wpct=XXX wlat=XXX min=XXX max=XXX"',
                    help='Use the specified QoS params instead of testing for them')
parser.add_argument('--quiet', action='store_true')
parser.add_argument('--verbose', action='store_true')

def err(msg):
    print(msg, file=sys.stderr)
    sys.stderr.flush()

def info(msg):
    if not args.quiet:
        print(msg, file=sys.stderr)
        sys.stderr.flush()

def dbg(msg):
    if args.verbose and not args.quiet:
        print(msg, file=sys.stderr)
        sys.stderr.flush()

# determine ('DEVNAME', 'MAJ:MIN') for @path
def dir_to_dev(path):
    # find the block device the current directory is on
    devname = subprocess.run(f'findmnt -nvo SOURCE -T{path}',
                             stdout=subprocess.PIPE, shell=True).stdout;
    devname = devname.decode('utf-8').strip();
    while os.path.islink(devname):
        link = os.readlink(devname)
        if not os.path.isabs(link):
            link = f'{os.path.dirname(devname)}/{link}';
        devname = os.path.realpath(link)

    if devname.startswith("/dev/dm") or devname.startswith("/dev/md"):
        info(f'{devname} is composite, you probably want to use --testfile-dev to specify the underlying device')

    devname = os.path.basename(devname)

    # partition -> whole device
    parents = glob.glob('/sys/block/*/' + devname)
    if len(parents):
        devname = os.path.basename(os.path.dirname(parents[0]))
    return devname

def create_testfile(path, size):
    global args

    if os.path.isfile(path) and os.stat(path).st_size == size:
        return

    subprocess.check_call(f'rm -f {path}', shell=True)
    subprocess.check_call(f'touch {path}', shell=True)
    subprocess.call(f'chattr +C {path}', shell=True)
    cmd = (f'dd if=/dev/urandom of={path} count={size} '
           f'iflag=count_bytes,fullblock oflag=direct bs=16M status=none')
    p = subprocess.Popen(cmd, shell=True)
    while p.poll() == None:
        try:
            cur = os.stat(path).st_size
            info(f'Creating {size/(1<<30):.2f}G testfile: {cur/size*100:.2f}%')
        except:
            pass
        time.sleep(1)
    p.wait()
    if p.returncode != 0:
        err(f"Failed to create testfile (exit code {p.returncode})")
        sys.exit(1)

def outfile_path(name):
    return f'iocost-coef-fio-output-{name}.json'

def run_fio(testfile, duration, iotype, iodepth, blocksize, jobs, rate_iops, outfile):
    global args

    eta = 'never' if args.quiet else 'always'
    cmd = (f'fio --direct=1 --ioengine=libaio --name=coef '
           f'--filename={testfile} --runtime={round(duration)} '
           f'--readwrite={iotype} --iodepth={iodepth} --blocksize={blocksize} '
           f'--eta={eta} --output-format json --output={outfile} '
           f'--time_based --numjobs={jobs}')
    if rate_iops is not None:
        cmd += f' --rate_iops={rate_iops}'
    if not sys.stderr.isatty():
        cmd += " | stdbuf -oL tr '\r' '\n'"
    if args.verbose:
        dbg(f'Running {cmd}')
    subprocess.check_call(cmd, shell=True)
    with open(outfile, 'r') as f:
        d = json.load(f)
    return sum(j['read']['bw_bytes'] + j['write']['bw_bytes'] for j in d['jobs'])

def restore_elevator_nomerges():
    global elevator_path, nomerges_path, elevator, nomerges

    info(f'Restoring elevator to {elevator} and nomerges to {nomerges}')
    with open(elevator_path, 'w') as f:
        f.write(elevator)
    with open(nomerges_path, 'w') as f:
        f.write(nomerges)

def read_lat(pct, table):
    best_err = 100
    best_lat = 0
    for p, lat in table.items():
        err = abs(float(p) - pct)
        if err < best_err:
            best_err = err
            best_lat = lat
    return best_lat / 1000   # convert to usecs

def is_ssd():
    global dev_path

    with open(f'{dev_path}/queue/rotational', 'r') as f:
        return int(f.read()) == 0

def sig_handler(_signo, _stack_frame):
    sys.exit(0)

def parse_override(ovrstr):
    ovr = {}
    for field in ovrstr.split():
        if '=' in field:
            (key, val) = field.split('=')
            ovr[key] = float(val)

    return ovr

def determine_model(args, testfile):
    seqio_blksz = args.seqio_block_mb * (2 ** 20)

    info('Determining wbps...')
    wbps = run_fio(testfile, args.duration, 'write', 1, seqio_blksz,
                   args.numjobs, None, outfile_path('wbps'))
    info(f'\nwbps={wbps}, determining rbps...')
    rbps = run_fio(testfile, args.duration, 'read', 1, seqio_blksz,
                   args.numjobs, None, outfile_path('rbps'))
    info(f'\nrbps={rbps}, determining wseqiops...')
    wseqiops = round(
        run_fio(testfile, args.duration, 'write', args.seq_depth, 4096,
                args.numjobs, None, outfile_path('wseqiops')) / 4096)
    info(f'\nwseqiops={wseqiops}, determining rseqiops...')
    rseqiops = round(
        run_fio(testfile, args.duration, 'read', args.seq_depth, 4096,
                args.numjobs, None, outfile_path('rseqiops')) / 4096)
    info(f'\nrseqiops={rseqiops}, determining wrandiops...')
    wrandiops = round(
        run_fio(testfile, args.duration, 'randwrite', args.rand_depth, 4096,
                args.numjobs, None, outfile_path('wrandiops')) / 4096)
    info(f'\nwrandiops={wrandiops}, determining rrandiops...')
    rrandiops = round(
        run_fio(testfile, args.duration, 'randread', args.rand_depth, 4096,
                args.numjobs, None, outfile_path('rrandiops')) / 4096)
    info(f'\nrrandiops={rrandiops}')

    return (rbps, rseqiops, rrandiops, wbps, wseqiops, wrandiops)

def determine_lat(args, testfile, rw, pct, randiops):
    if rw == 'read':
        (rw, io_type, lat_type) = ('read', 'randread', 'rlat')
    elif rw == 'write':
        (rw, io_type, lat_type) = ('write', 'randwrite', 'wlat')
    else:
        raise Exception(f'invalid rw value "{rw}"')

    info(f'Determining {rw} QoS params...')

    run_fio(testfile, args.duration, io_type, args.rand_depth, 4096,
            args.numjobs, int(randiops * 0.9), outfile_path(lat_type))

    with open(outfile_path(lat_type), 'r') as f:
        r = json.load(f)
        lat = read_lat(pct, r['jobs'][0][rw]['clat_ns']['percentile'])

    return round(lat * 1.5)

def determine_qos(args, testfile, rrandiops, wrandiops):
    if is_ssd():
        rpct = 95
        wpct = 95
    else:
        rpct = 50
        wpct = 50

    rlat = determine_lat(args, testfile, 'read', rpct, rrandiops)
    wlat = determine_lat(args, testfile, 'write', wpct, wrandiops)

    return (rpct, rlat, wpct, wlat, 60, 100)

#
# Execution starts here
#
signal.signal(signal.SIGTERM, sig_handler)
signal.signal(signal.SIGINT, sig_handler)
args = parser.parse_args()

missing = False
for cmd in [ 'findmnt', 'dd', 'fio', 'stdbuf' ]:
    if not shutil.which(cmd):
        err(f'Required command "{cmd}" is missing')
        missing = True
if missing:
    sys.exit(1)

if args.testdev:
    devname = os.path.basename(args.testdev)
    rdev = os.stat(f'/dev/{devname}').st_rdev
    devnr = f'{os.major(rdev)}:{os.minor(rdev)}'
    testfile = f'/dev/{devname}'
    info(f'Test target: {devname}({devnr})')
else:
    if args.testfile_dev:
        devname = os.path.basename(args.testfile_dev)
    else:
        devname = dir_to_dev('.')

    devpath = f'/dev/{devname}'

    if not os.path.exists(devpath):
        err(f'{devpath} does not exist, use --testfile-dev')
        sys.exit(1)

    rdev = os.stat(devpath).st_rdev
    devnr = f'{os.major(rdev)}:{os.minor(rdev)}'
    testfile = 'iocost-coef-fio.testfile'
    testfile_size = int(args.testfile_size_gb * 2 ** 30)
    create_testfile(testfile, testfile_size)
    info(f'Test target: {testfile} on {devname}({devnr})')

dev_path = f'/sys/block/{devname}'
elevator_path = f'{dev_path}/queue/scheduler'
nomerges_path = f'{dev_path}/queue/nomerges'

with open(elevator_path, 'r') as f:
    elevator = re.sub(r'.*\[(.*)\].*', r'\1', f.read().strip())
with open(nomerges_path, 'r') as f:
    nomerges = f.read().strip()

info(f'Temporarily disabling elevator and merges')
atexit.register(restore_elevator_nomerges)
with open(elevator_path, 'w') as f:
    f.write('none')
with open(nomerges_path, 'w') as f:
    f.write('1')

if args.model_override is None:
    (rbps, rseqiops, rrandiops, wbps, wseqiops, wrandiops) = \
        determine_model(args, testfile)
else:
    ovr = parse_override(args.model_override)
    (rbps, rseqiops, rrandiops, wbps, wseqiops, wrandiops) = \
        (ovr['rbps'], ovr['rseqiops'], ovr['rrandiops'],
         ovr['wbps'], ovr['wseqiops'], ovr['wrandiops'])

if args.qos_override is None:
    (rpct, rlat, wpct, wlat, vmin, vmax) = \
        determine_qos(args, testfile, rrandiops, wrandiops)
else:
    ovr = parse_override(args.qos_override)
    (rpct, rlat, wpct, wlat, vmin, vmax) = \
        (ovr['rpct'], ovr['rlat'], ovr['wpct'], ovr['wlat'],
         ovr['min'], ovr['max'])

restore_elevator_nomerges()
atexit.unregister(restore_elevator_nomerges)

if args.json:
    result = {
        'devnr': devnr,
        'model': {
            'rbps': rbps,
            'rseqiops': rseqiops,
            'rrandiops': rrandiops,
            'wbps': wbps,
            'wseqiops': wseqiops,
            'wrandiops': wrandiops,
        },
        'qos': {
            'rpct': rpct,
            'rlat': rlat,
            'wpct': wpct,
            'wlat': wlat,
            'min': vmin,
            'max': vmax,
        },
    }

    info(f'Writing results to {args.json}')
    with open(args.json, 'w') as f:
        json.dump(result, f, indent=4)

info('')
print(f'io.cost.model: {devnr} rbps={rbps} rseqiops={rseqiops} '
      f'rrandiops={rrandiops} wbps={wbps} wseqiops={wseqiops} '
      f'wrandiops={wrandiops}')
print(f'io.cost.qos: {devnr} rpct={rpct} rlat={rlat} '
      f'wpct={wpct} wlat={wlat} min={vmin} max={vmax}')

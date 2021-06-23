#!/bin/python3
# Copyright (c) Facebook, Inc. and its affiliates

import os
import mmap
import time
import math
import sys

GIG = 1024 * 1024 * 1024
NR_FILES = 1000
TF_DIR = "inodesteal-testfiles"
CHUNK_PAGES = int((256 << 20) / 4096)
CHUNK_SIZE = CHUNK_PAGES * 4096
NR_MMS_TO_GIGS = CHUNK_SIZE / GIG
TRIES = 3

def read_meminfo():
    meminfo = {}
    with open("/proc/meminfo", "r") as f:
        for line in f.readlines():
            toks = line.replace(":", " ").split()
            if len(toks) < 3:
                meminfo[toks[0]] = int(toks[1])
            elif toks[2] == "kB":
                meminfo[toks[0]] = int(toks[1]) * 1024
    return meminfo

def read_vmstat():
    vmstat = {}
    with open("/proc/vmstat", "r") as f:
        for line in f.readlines():
            toks = line.split()
            vmstat[toks[0]] = int(toks[1])
    return vmstat

def read_inodesteal():
    vmstat = read_vmstat()
    return vmstat["pginodesteal"] + vmstat["kswapd_inodesteal"]

def one_round(inodesteal_target, prefix):
    with open("/proc/sys/vm/drop_caches", "w") as f:
        f.write("3")

    mi = read_meminfo()
    mem_free = mi["MemFree"]
    swap_free = mi["SwapFree"]
    target_swap = min(1 << 30, swap_free / 2)
    target_swap_free = swap_free - target_swap

    print(f"{prefix}mem_free={mem_free/GIG:.2f}G swap_free={swap_free/GIG:.2f}G target_swap={target_swap/GIG:.2f}G",
          file=sys.stderr)

    # Instantiate inode cache and revisit to activate.
    for i in range(2):
        for j in range(NR_FILES):
            with open(TF_DIR + f"/{j}", "r") as f:
                f.read()

    # Add some inactive page cache.
    with open(TF_DIR + "/inactive", "w+") as f:
        f.truncate(2 * target_swap)
        f.read()

    # Balloon up in 256M increments until swap_free falls below target_swap_free.
    last_at = time.time()
    mms = []
    while True:
        mm = mmap.mmap(-1, CHUNK_PAGES * 4096, flags=mmap.MAP_PRIVATE)
        for i in range(CHUNK_PAGES):
            mm[i * 4096] = 1
        mi = read_meminfo()
        mms.append(mm)
        if mi["SwapFree"] < target_swap_free:
            break
        if time.time() >= last_at + 1:
            print(f"{prefix}Allocated {len(mms) * NR_MMS_TO_GIGS:.2f}G swap_free={mi['SwapFree'] / GIG:.2f}G",
                  file=sys.stderr)
            last_at = time.time()

    print(f"{prefix}Finished allocating {len(mms) * NR_MMS_TO_GIGS:.2f}G swap_free={mi['SwapFree'] / GIG:.2f}G",
          file=sys.stderr)

    # Give some of the mmapped pages a round of read. We just wanna dip into
    # swap a bit. Read half of the target swap usage.
    last_at = time.time()
    nr_to_read = min(math.ceil(2 * target_swap / CHUNK_SIZE / 2), len(mms))
    for i in range(nr_to_read):
        for j in range(CHUNK_PAGES):
            mms[i][j * 4096]
        if read_inodesteal() >= inodesteal_target:
            return True
        if time.time() >= last_at + 1:
            print(f"{prefix}Accessed {i * NR_MMS_TO_GIGS:.2f}G",
                  file=sys.stderr)

    return False

#
# Control starts here.
#

# First, populate the testfiles.
os.makedirs(TF_DIR, exist_ok=True)
for i in range(NR_FILES):
    with open(TF_DIR + f"/{i}", "w") as f:
        f.truncate(4096)
os.sync()

# Determine the target inodesteal count.
vmstat = read_vmstat()
inodesteal_target = read_inodesteal() + NR_FILES / 10

for i in range(TRIES):
    if one_round(inodesteal_target, f"RUN {i+1}/{TRIES}: "):
        print("FAIL - shadow inode entries are not protected")
        sys.exit(1)

with open("inodesteal-success-at", "w") as f:
    f.write(str(time.time()))

print("SUCCESS - shadow inode entries are protected")

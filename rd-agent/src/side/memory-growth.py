#!/bin/python3
# Copyright (c) Facebook, Inc. and its affiliates

import os
import sys
import math
import mmap
import time

if len(sys.argv) < 3:
    print('Usage: memory-balloon.py RBPS WBPS', file=sys.stderr);
    sys.exit(1)

if sys.argv[1][-1] == '%':
    rbps = float(os.environ.get('IO_RBPS')) * float(sys.argv[1][:-1]) / 100
else:
    rbps = float(sys.argv[1])

if sys.argv[2][-1] == '%':
    wbps = float(os.environ.get('IO_WBPS')) * float(sys.argv[2][:-1]) / 100
else:
    wbps = float(sys.argv[2])

PAGE_SZ = 4096
SWAP_CLUSTER_MAX = 32
CHUNK_SZ = 128 << 20
MAX_DEBT_DUR = 10

chunks = []
read_sz = 0
write_sz = 0
rdebt = rbps
wdebt = wbps

def alloc_next_page():
    global chunks, write_sz

    write_sz += PAGE_SZ
    idx = int(write_sz / CHUNK_SZ)
    off = write_sz % CHUNK_SZ

    if idx == len(chunks):
        chunks.append(mmap.mmap(-1, CHUNK_SZ, flags=mmap.MAP_PRIVATE))

    chunks[idx][off] = 1

def read_next_page():
    global chunks, read_sz, write_sz

    if write_sz == 0:
        return

    addr = read_sz % write_sz
    idx = int(addr / CHUNK_SZ)
    off = addr % CHUNK_SZ

    v = chunks[idx][off]

    read_sz += PAGE_SZ

def run():
    global rbps, wbps, chunks, read_sz, write_sz, rdebt, wdebt

    print('Target rbps={:.2f}M wbps={:.2f}M'
          .format(rbps / (1 << 20), wbps / (1 << 20), flush=True))

    debt_at = time.time()
    report_at = debt_at

    last_read = 0
    last_write = 0

    while True:
        cnt = 0
        while wdebt >= PAGE_SZ and cnt < SWAP_CLUSTER_MAX:
            alloc_next_page()
            wdebt -= PAGE_SZ
            cnt += 1

        cnt = 0
        while rdebt >= PAGE_SZ and cnt < math.ceil(SWAP_CLUSTER_MAX * rbps / wbps):
            read_next_page()
            rdebt -= PAGE_SZ
            cnt += 1

        now = time.time()
        if now - debt_at > 0.1:
            dur = now - debt_at
            rdebt = min(rdebt + rbps * dur, rbps * MAX_DEBT_DUR)
            wdebt = min(wdebt + wbps * dur, wbps * MAX_DEBT_DUR)
            debt_at += dur

        if now - report_at > 1:
            dur = now - report_at
            print('size={:.2f}G rbps={:.2f}M wbps={:.2f}M'
                  .format(write_sz / (1 << 30),
                          ((read_sz - last_read) / (1 << 20)) / dur,
                          ((write_sz - last_write) / (1 << 20)) / dur),
                  flush=True)
            last_read = read_sz
            last_write = write_sz
            report_at += dur

        if rdebt < PAGE_SZ and wdebt < PAGE_SZ:
            time.sleep(0.1)

if __name__ == "__main__":
    run()

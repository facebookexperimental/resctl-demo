#!/bin/python3
# Copyright (c) Facebook, Inc. and its affiliates

import datetime
import gc
import os
import resource
import sys
import time

BPS = int(sys.argv[1]) << 20
PAGE_SIZE = resource.getpagesize()

def get_memory_usage():
    return int(open("/proc/self/statm", "rt").read().split()[1]) * PAGE_SIZE

def bloat(size):
    l = []
    mem_usage = get_memory_usage()
    target_mem_usage = mem_usage + size
    while get_memory_usage() < target_mem_usage:
        l.append(b"g" * (10 ** 6))
    return l

def run():
    arr = []  # prevent GC
    prev_time = datetime.datetime.now()
    while True:
        # allocate some memory
        l = bloat(BPS)
        arr.append(l)
        now = datetime.datetime.now()
        print("{} -- RSS = {} bytes. Delta = {}"
              .format(now, get_memory_usage(), (now - prev_time).total_seconds()),
              flush=True)
        prev_time = now
        time.sleep(1)

    print('{} -- Done with workload'.format(datetime.datetime.now()))

if __name__ == "__main__":
    run()

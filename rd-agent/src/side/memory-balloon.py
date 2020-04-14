#!/bin/python3
# Copyright (c) Facebook, Inc. and its affiliates

import sys
import mmap
import time

if len(sys.argv) < 2:
    print('Usage: memory-balloon.py BYTES', file=sys.stderr);

nr_pages = int((int(sys.argv[1]) + 4095) / 4096)
mm = mmap.mmap(-1, nr_pages * 4096, flags=mmap.MAP_PRIVATE)

last_at = time.time()

for i in range(nr_pages):
    mm[i*4096] = 1
    if time.time() >= last_at + 1 or i == nr_pages - 1:
        print(f'Touched {i * 4096 / (1 << 30):.2}G')
        last_at = time.time()

print("Allocation done, sleeping...")
while True:
    time.sleep(600)

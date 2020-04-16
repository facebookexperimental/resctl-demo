#!/bin/python3
# Copyright (c) Facebook, Inc. and its affiliates

import os
import sys
import math
import subprocess

JOBS=8
MIN_DEPTH_PER_JOB=8

if len(sys.argv) < 2:
   print('Usage: read-bomb.py DEPTH [READ_SIZE]', file=sys.stderr);
   sys.exit(1)

dev = '/dev/' + os.environ.get('IO_DEV')
depth = int(sys.argv[1])
if len(sys.argv) >= 3:
   size = int(sys.argv[2])
else:
   size = 4096

jobs = min(JOBS, math.ceil(depth / MIN_DEPTH_PER_JOB))
depth = math.ceil(depth / jobs)

print(f'Reading {size/1024}k with {depth} depth and {jobs} jobs from {dev}', flush=True)

cmd = (f"fio --direct=1 --ioengine=libaio --name=read-bomb "
       f"--filename={dev} --readwrite=randread --iodepth={depth} --blocksize={size} "
       f"--numjobs={jobs} --eta=always --eta-interval=1 | stdbuf -oL tr '\r' '\n'")
print(f'Running \"{cmd}\"', flush=True)

subprocess.check_call(cmd, shell=True)

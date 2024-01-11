#!/usr/bin/python3
# Copyright (c) Facebook, Inc. and its affiliates

import mmap
import os
import sys
import time
import socket


if len(sys.argv) < 2:
    print("Usage: memory-balloon.py BYTES", file=sys.stderr)
    sys.exit(1)

try:
    sd_socket = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    sd_addr = os.getenv("NOTIFY_SOCKET")
    assert sd_addr, "$NOTIFY_SOCKET not available"
    sd_socket.connect(sd_addr)
except Exception:
    print("Failed to create systemd socket")
    raise

nr_pages = int((int(sys.argv[1]) + 4095) / 4096)
mm = mmap.mmap(-1, nr_pages * 4096, flags=mmap.MAP_PRIVATE)

last_at = time.time()

for i in range(nr_pages):
    mm[i * 4096] = 1
    if time.time() >= last_at + 1 or i == nr_pages - 1:
        print(f"Touched {i * 4096 / (1 << 30):.2f}G")
        last_at = time.time()

try:
    sd_socket.sendall(b"READY=1")
except Exception:
    print("Failed to send ready notification to systemd")
    raise

print("Allocation done, sleeping...")
while True:
    time.sleep(600)

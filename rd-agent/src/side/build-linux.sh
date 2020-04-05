#!/bin/bash
# Copyright (c) Facebook, Inc. and its affiliates

set -xe

NR_JOBS=
if [ -n "$1" ]; then
    NR_JOBS=$(nproc)
    NR_JOBS=$((NR_JOBS * $1))
    if [ -n "$2" ]; then
        NR_JOBS=$((NR_JOBS / $2))
    fi
    NR_JOBS=$(((NR_JOBS * 12 + 9) / 10))
fi

rm -rf linux-*
tar xvf ../../linux.tar
cd linux-*
make allmodconfig
make -j$NR_JOBS

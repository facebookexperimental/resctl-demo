#!/bin/bash
# Copyright (c) Facebook, Inc. and its affiliates

set -xe

NR_JOBS=
if [ -n "$2" ]; then
    NR_JOBS=$((NR_CPUS * $2))
    if [ -n "$3" ]; then
        NR_JOBS=$((NR_JOBS / $3))
    fi
    NR_JOBS=$(((NR_JOBS * 12 + 9) / 10))
fi

rm -rf linux-*
tar xvf ../../linux.tar
cd linux-*
make "$1"

STARTED_AT=$(date +%s)
make -j$NR_JOBS
ENDED_AT=$(date +%s)

echo "Compilation took $((ENDED_AT-STARTED_AT)) seconds"

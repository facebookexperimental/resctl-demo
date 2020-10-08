#!/bin/bash
# Copyright (c) Facebook, Inc. and its affiliates

set -e

NR_JOBS=$((NR_CPUS * $1))
if [ -n "$2" ]; then
    NR_JOBS=$((NR_JOBS / $2))
fi

echo "Saturating CPUs with $NR_JOBS threads..."

stress --cpu $NR_JOBS

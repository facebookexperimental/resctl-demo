#!/bin/bash
#
# Support installations which have bcc available only through py-bcc.
#
# Copyright (c) Facebook, Inc. and its affiliates

IO_LAT="$(dirname "$0")/io_latencies.py"

if command -v bcc-py >/dev/null; then
    bcc-py "$IO_LAT" "$@"
else
    "$IO_LAT" "$@"
fi

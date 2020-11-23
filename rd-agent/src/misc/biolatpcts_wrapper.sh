#!/bin/bash
#
# Support installations which have bcc available only through py-bcc.
#
# Copyright (c) Facebook, Inc. and its affiliates

IO_LAT="$(dirname "$0")/biolatpcts.py"

if command -v bcc-py >/dev/null; then
    exec bcc-py "$IO_LAT" "$@"
else
    exec "$IO_LAT" "$@"
fi

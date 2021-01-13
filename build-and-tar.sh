#!/bin/bash
# Copyright (c) Facebook, Inc. and its affiliates
set -e

BINS=("rd-hashd" "rd-agent" "resctl-demo" "resctl-bench")
BUILT_BINS=("${BINS[@]/#/target/release/}")

DIR=$(dirname "$0")
cd "$DIR"

cargo build --release "${@}"

echo "[ Creating target/resctl-demo.tar.gz ]"
tar cvzf target/resctl-demo.tar.gz --transform 's|^.*/\([^/]*\)$|\1|' ${BUILT_BINS[@]}

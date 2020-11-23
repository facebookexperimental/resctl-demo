#!/bin/bash
# Copyright (c) Facebook, Inc. and its affiliates
set -e

DIR=$(dirname "$0")
cd "$DIR"

if ! [ -f target/release/rd-hashd ] || \
       ! [ -f target/release/rd-agent ] || \
       ! [ -f target/release/resctl-demo ] || \
       ! [ -f target/release/resctl-bench ]; then
    echo 'Error: Binaries not ready. Run "cargo build --release".' 2>&1
    exit 1
fi

echo "[ Creating target/resctl-demo.tar.gz ]"
tar cvzf target/resctl-demo.tar.gz --transform 's|^.*/\([^/]*\)$|\1|' \
    target/release/rd-hashd \
    target/release/rd-agent \
    target/release/resctl-demo \
    target/release/resctl-bench

if [ -n "$1" ]; then
    echo "[ Installing under $1 ]"
    cd "$1"; tar xzf target/resctl-demo.tar.gz
else
    echo "[ Target directory not specified ]"
fi

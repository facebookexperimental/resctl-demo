#!/bin/bash
# Copyright (c) Facebook, Inc. and its affiliates
set -e

DIR=$(dirname $0)
cd "$DIR"

if ! [ -f target/release/rd-hashd -a \
       -f target/release/rd-agent -a \
       -f target/release/resctl-demo ]; then
    echo 'Error: Binaries not ready. Run "cargo build --release".' 2>&1
    exit 1
fi

echo "[ Creating target/resctl-demo.tar.gz ]"
tar cvzf target/resctl-demo.tar.gz --transform 's|^.*/\([^/]*\)$|\1|' \
    target/release/rd-hashd \
    target/release/rd-agent \
    target/release/resctl-demo

if [ -n "$1" ]; then
    echo "[ Installing under $1 ]"
    cat target/resctl-demo.tar.gz | (cd $1; tar xzf -)
else
    echo "[ Target directory not specified ]"
fi

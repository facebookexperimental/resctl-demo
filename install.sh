#!/bin/bash
# Copyright (c) Facebook, Inc. and its affiliates
set -x

if ! [ -f target/release/rd-hashd -a \
       -f target/release/rd-agent -a \
       -f target/release/resctl-demo ]; then
    echo 'Run "cargo build --release"'
    exit 1
fi

cp -f target/release/rd-hashd \
      target/release/rd-agent \
      target/release/resctl-demo \
      misc/iocost_coef_gen.py \
      misc/sideloader.py \
      /usr/local/bin

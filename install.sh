#!/bin/bash

if ! [ -f target/release/rd-hashd -a -f target/release/rd-agent ]; then
    echo 'Run "cargo build --release"'
    exit 1
fi

cp -f target/release/rd-hashd \
   target/release/rd-agent \
   misc/iocost_coef_gen.py \
   misc/sideloader.py \
   /usr/local/bin

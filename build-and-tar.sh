#!/bin/bash
# Copyright (c) Facebook, Inc. and its affiliates
set -e

BINS=("rd-hashd" "rd-agent" "resctl-demo" "resctl-bench")
INSTALL_ROOT="target/install-root"
BUILT_BINS=("${BINS[@]/#/${INSTALL_ROOT}/bin/}")

DIR=$(dirname "$0")
cd "$DIR"

for i in "${BINS[@]}"; do
    cargo install --root ${INSTALL_ROOT} "${@}" --path $i
done

echo "[ Creating target/resctl-demo.tar.gz ]"
tar cvzf target/resctl-demo.tar.gz --transform 's|^.*/\([^/]*\)$|\1|' ${BUILT_BINS[@]}

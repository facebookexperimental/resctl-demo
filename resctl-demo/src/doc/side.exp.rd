## Copyright (c) Facebook, Inc. and its affiliates.
%% id side.exp: Experiment with Sideloading
%% reset secondaries
%% reset protections
%% knob sys-cpu-ratio 0.01
%% knob sys-io-ratio 0.01
%% knob hashd-load 0.6
%% on hashd
$$ reset resctl-params

*Experiment with Sideloading*\n
*===========================*

Play with the parameters and workloads, and see what happens.

%% toggle hashd                  : hashd toggle
%% knob hashd-load               : hashd load   :
%% knob cpu-headroom             : CPU headroom :

Sideload toggles:

%% toggle sideload build-linux build-linux-2x : allmodconfig linux build
%% toggle sideload build-linux-min build-linux-allnoconfig-2x : allnoconfig linux build
%% toggle sideload build-linux-def build-linux-defconfig-2x : allmodconfig linux build
%% toggle sideload memory-bomb memory-growth-25pct : Memory bomb
%% toggle sideload io-bomb read-bomb : IO bomb

Sysload toggles:

%% toggle sysload build-linux build-linux-2x : allmodconfig linux build
%% toggle sysload build-linux-min build-linux-allnoconfig-2x : allnoconfig linux build
%% toggle sysload build-linux-def build-linux-defconfig-2x : allmodconfig linux build
%% toggle sysload memory-bomb memory-growth-25pct : Memory bomb
%% toggle sysload io-bomb read-bomb : IO bomb


___*Read on*___

%% jump senpai.intro             : [ Next: The Problem of Sizing Memory ]

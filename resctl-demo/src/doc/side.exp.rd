## Copyright (c) Facebook, Inc. and its affiliates.
%% id side.exp: Experiment with Sideloading
%% reset prep
%% knob sys-cpu-ratio 0.01
%% knob sys-io-ratio 0.01
%% knob hashd-load 0.6
%% on hashd

*Experiment with Sideloading*\n
*===========================*

Play with the parameters and workloads, and see what happens.

%% toggle hashd                  : hashd toggle
%% knob hashd-load               : hashd load   :
%% knob cpu-headroom             : CPU headroom :

Sideload toggles:

%% toggle sideload compile-job build-linux-1x : allmodconfig linux build
%% toggle sideload compile-job-1 build-linux-allnoconfig-1x : allnoconfig linux build
%% toggle sideload compile-job-2 build-linux-defconfig-1x : allmodconfig linux build
%% toggle sideload memory-hog memory-growth-50pct : Memory hog
%% toggle sideload io-hog read-bomb : IO hog

Sysload toggles:

%% toggle sysload compile-job build-linux-1x : allmodconfig linux build
%% toggle sysload compile-job-1 build-linux-allnoconfig-1x : allnoconfig linux build
%% toggle sysload compile-job-2 build-linux-defconfig-1x : allmodconfig linux build
%% toggle sysload memory-hog memory-growth-50pct : Memory hog
%% toggle sysload io-hog read-bomb : IO hog


___*Read on*___

%% jump senpai.intro             : [ Next: The Problem of Sizing Memory ]

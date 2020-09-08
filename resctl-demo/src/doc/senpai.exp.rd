## Copyright (c) Facebook, Inc. and its affiliates.
%% id senpai.exp: Senpai Playground
%% reset secondaries
%% reset protections

*Senpai Playground*\n
*=================*

***WARNING: Senpai does not work on rotating hard disks.***

Senpai toggles:

%% toggle oomd-work-senpai       : [ Senpai on workload.slice ]
%% toggle oomd-sys-senpai        : [ Senpai on system.slice ]

Play with the parameters and workloads, and see what happens.

%% toggle hashd                  : hashd toggle
%% knob hashd-load               : hashd load   :

Sysload toggles:

%% toggle sysload build-linux build-linux-2x : allmodconfig linux build
%% toggle sysload build-linux-min build-linux-allnoconfig-2x : allnoconfig linux build
%% toggle sysload build-linux-def build-linux-defconfig-2x : allmodconfig linux build
%% toggle sysload memory-bomb memory-growth-25pct : Memory bomb
%% toggle sysload io-bomb read-bomb : IO bomb


___*Read on*___

%% jump index                    : [ Exit: Index ]

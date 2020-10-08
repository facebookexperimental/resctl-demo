## Copyright (c) Facebook, Inc. and its affiliates.
%% id senpai.exp: Senpai Playground
%% reset prep

*Senpai Playground*\n
*=================*

___***WARNING***___: Senpai does not work on rotating hard disks.

Senpai toggles:

%% toggle oomd-work-senpai       : [ Senpai on workload.slice ]
%% toggle oomd-sys-senpai        : [ Senpai on system.slice ]

Play with the parameters and workloads, and see what happens.

%% toggle hashd                  : hashd toggle
%% knob hashd-load               : hashd load   :

Sysload toggles:

%% toggle sysload compile-job build-linux-2x : allmodconfig linux build
%% toggle sysload compile-job-1 build-linux-allnoconfig-2x : allnoconfig linux build
%% toggle sysload compile-job-2 build-linux-defconfig-2x : allmodconfig linux build
%% toggle sysload memory-hog memory-growth-50pct : Memory hog
%% toggle sysload io-hog read-bomb : IO hog


___*Read on*___

%% jump credits                  : [ Next: Credits ]

## Copyright (c) Facebook, Inc. and its affiliates.
%% id prot.demo: Comprehensive Protection Demo
%% reset secondaries
%% reset protections
$$ reset hashd-params
$$ reset resctl-params
$$ reset secondaries

*Throwing Everything At It*\n
*=========================*

Play with various workloads and protection controls and see what happens.

rd-hashd control:

%% toggle hashd                  : Enable
%% knob   hashd-load             : Load level       :
%% knob   hashd-mem              : Memory footprint :
%% reset  hashd-params           : [ Restore default parameters ]

Protection settings:

%% toggle mem-resctl             : Memory protection
%% toggle io-resctl              : IO protection
%% toggle cpu-resctl             : CPU protection
%% knob   sys-cpu-ratio          : System CPU weight compared to rd-hashd :
%% knob   sys-io-ratio           : System IO weight compared to rd-hashd  :
%% reset  resctl-params          : [ Restore default parameters ]

Workloads for system.slice:

%% toggle sysload build-linux build-linux-4x      : Build Linux
%% toggle sysload memory-bomb-50pct memory-growth-50pct : Cold memory growth (50% of max write bw)
%% toggle sysload memory-bomb-1x memory-growth-1x : Cold memory growth (1x of max write bw)
%% toggle sysload memory-bomb-2x memory-growth-2x : Cold memory growth (2x of max write bw)
%% toggle sysload memory-bloat memory-bloat-1x    : Hot memory growth
%% toggle sysload read-bomb read-bomb             : Concurrent read bomb
%% reset  secondaries                             : [ Reset all system.slice workloads ]


___*Read on*___

Now that we explored resource protection. Let's take a look at something
more exciting - sideloading.

%% jump side.intro               : [ Next: What Is Sideloading? ]
%% jump comp.oomd                : [ Prev: OOMD - The Out-Of-Memory Daemon ]

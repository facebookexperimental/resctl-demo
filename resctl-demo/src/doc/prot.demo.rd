## Copyright (c) Facebook, Inc. and its affiliates.
%% id prot.demo: Comprehensive Protection Demo
%% reset prep
$$ reset all-with-params

*Throwing Everything At It*\n
*=========================*

Play with various workloads and protection controls and see what happens.

rd-hashd control:

%% toggle hashd                  : Enable
%% knob   hashd-load             : Load level          :
%% knob   hashd-mem              : Memory footprint    :
%% knob   hashd-lat-target       : Target latency (ms) :
%% knob   hashd-lat-target-pct   : Latency percentile  :
%% reset  hashd-params           : [ Restore default parameters ]

Protection settings:

%% toggle mem-resctl             : Memory protection
%% toggle io-resctl              : IO protection
%% toggle cpu-resctl             : CPU protection
%% knob   sys-cpu-ratio          : System CPU weight compared to rd-hashd :
%% knob   sys-io-ratio           : System IO weight compared to rd-hashd  :
%% reset  resctl-params          : [ Restore default parameters ]

Workloads for ___system___:

%% toggle sysload compile-job    build-linux-2x      : Compile Linux (2x CPUs)
%% toggle sysload compile-job-1  build-linux-4x      : Compile Linux (4x CPUs)
%% toggle sysload compile-job-2  build-linux-16x     : Compile Linux (16x CPUs)
%% toggle sysload compile-job-3  build-linux-32x     : Compile Linux (32x CPUs)
%% toggle sysload memory-hog     memory-growth-50pct : Cold memory hog (50% of max write bw)
%% toggle sysload memory-hog-1   memory-growth-1x    : Cold memory hog (1x of max write bw)
%% toggle sysload memory-hog-hot memory-bloat-1x     : Hot memory hog
%% toggle sysload io-hog read-bomb                   : IO hog - concurrent reads
%% toggle sysload cpu-hog burn-cpus-50pct            : CPU hog (50% of CPU threads)
%% toggle sysload cpu-hog-1 burn-cpus-1x             : CPU hog (1x of CPU threads)
%% toggle sysload cpu-hog-2 burn-cpus-2x             : CPU hog (2x of CPU threads)
%% reset  secondaries                                : [ Reset all system.slice workloads ]


___*Read on*___

Now that we explored resource protection. Let's take a look at something
more exciting - sideloading.

%% jump side.intro               : [ Next: What Is Sideloading? ]

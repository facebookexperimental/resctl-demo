## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.cgroup.cpu: CPU Control
%% reset secondaries
%% reset protections

*CPU Control*\n
*===========*

First, let's get hashd running full tilt so that it's all warmed up later
when we want to test it.

%% (                             : [ Start hashd at full load ]
%% knob hashd-load 1.0
%% on hashd
%% )

CPU control is conceptually straight-forward. If a workload doesn't get
sufficient CPU cycles, it won't be able to perform its job. CPU usage is
primarily measured in wallclock time and the cgroup CPU controller can
distribute proportinally with `cpu.weight` or limit absolute consumption
with `cpu.max`.

The picture gets muddier up close. While wallclock time captures the
utilization to a reasonable degree, CPU time is an aggregate measurement
encompassing on-CPU compute and cache resources, memory bandwidth and more,
each of which has its own performance characteristics.

As the CPUs get close to saturation, all the components get more bogged down
and the increase in total amount of work done significantly lags behind the
increase in CPU time. Further muddling the picture, many of the above
components are shared across CPU cores and logical threads (hyperthreading)
and how they're distributed by CPU impacts resource distribution.

There are kernel-side complications too.


___*Read on*___

%% jump comp.cgroup              : [ Up: cgroup and Resource Protection ]
%%
%% jump index                    : [ Exit: Index ]

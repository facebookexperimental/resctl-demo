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

CPU control is relatively straight-forward. If a workload doesn't get
sufficient CPU cycles, it won't be able to perform its job. CPU usage is
primarily measured in wallclock time and the cgroup CPU controller can
distribute proportinally with `cpu.weight` or limit absolute consumption
with `cpu.max`. In most cases, configuring `cpu.weight` at higher level
cgroups is sufficient.

Up close, the picture gets a bit muddy. While wallclock time captures the
utilization to a reasonable degree, CPU time is an aggregate measurement
encompassing on-CPU compute and cache resources, memory bandwidth and more,
each of which has its own performance characteristics.

As the CPUs get close to saturation, all the components get more bogged down
and the increase in total amount of work done significantly lags behind the
increase in CPU time. Further muddling the picture, many of the above
components are shared across CPU cores and logical threads (hyperthreading)
and how they're distributed by CPU impacts resource distribution. This has
implications on sideloading and will be re-visited there.

`cpu.weight` currently repeats scheduling per each level of cgroup tree. For
scheduling-intensive workloads, the overhead can add up to a noticeable
amount as the nesting level grows. Unfortunately, the only solution
currently is limiting the level that CPU controller is enabled. systemd's
"DisableControllers" option can be useful for this purpose.


___*`cpu.max` and priority inversions*___

One of the reasons why priority inversions aren't crippling problems for
Linux and most other operating systems is that they're usually self-solving.
When a low priority process ends up blocking the whole system, the system
soon runs out of things to do and the blocking process has the whole machine
to finish what it was doing and unblock others. This effectively works as a
innate crude priority inheritance mechanism. However, this only works when
the system doesn't put strict upper limits on parts of the system.

Let's say the same low priority process is under stingy `cpu.max` limit and
it somehow ends up blocking a big portion of the system maybe through a
kernel mutex. While the rest of the system keeps piling up on the mutex and
the system as a whole is going idle, the low priority process cannot run
because it doesn't have enough CPU budget.

While future kernels may improve handling of this particular situation, this
is a repeating theme throughout resource control - the stricter resource
utilization capping, the liklier priority inversions and system-wide hangs
become. Work-conserving resource control mechanisms are easier to use, more
forgiving in terms of configuration accuracy and way safer, because it
doesn't reduce the total amount of work the system does and thus still keeps
most of the benefits of the innate priority inheritance behavior.

Unless absolutely necessary, stick with `cpu.weight`. When you have to use
`cpu.max`, avoid limiting it too harshly to avoid system-wide hangs.


___*What about cpuset?*___

cpuset can be useful in some circumstances. However, it is limited in
control granularity often requiring manual configurations and shares similar
problems with `cpu.max`. As the CPUs that a given workload may run on gets
further restricted, the possibility of priority inversion events which can
lead to system level events which can impact latency profile and lower
utilization becomes higher.

While `cpuset` is useful as further optimization, `cpu.weight` is better
suited as generic system-wide CPU control mechanism and will be the primary
focus in this demo.


___*Let's see it working*___

If hashd isn't running yet, start it up and wait for it to ramp up.

%% (                             : [ Start hashd at full load ]
%% knob hashd-load 1.0
%% on hashd
%% )

Once hashd is warmed up, let's disable CPU control and start a linux build
job with concurrency of twice the CPU thread count, which is high but not
outrageous.

%% (                             : [ Disable CPU control and start building kernel ]
%% off cpu-resctl
%% on sysload build-linux build-linux-2x
%% )

As the compile jobs ramp up, RPS will decline. It won't crater but the
situation is still far from acceptable. Let's re-enable CPU control.

%% (                             : [ Restore CPU control ]
%% reset protections
%% )

Note that RPS will recover but is still noticeably lower than ~90% which is
where it should be given the 10:1 cpu weight ratio. This is partially the
effect of the above described muddiness around CPU time. We'll revisit this
later when discussing sideloading.


___*Read on*___

For working resource control, it's critical to understand which resources
are being contended for. While cgroup provides resource utilization
monitoring, it's impossible to understand resource shortages from
utilization information. PSI provides the critical insight into resource
contention both at system and per-cgroup level.

%% jump comp.psi                 : [ Next: Monitoring Resource Contention with PSI ]
%% jump index                    : [ Exit: Index ]

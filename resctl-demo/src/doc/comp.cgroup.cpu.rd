## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.cgroup.cpu: CPU Control
%% reset prep
%% knob hashd-load 1.0
%% on hashd

*CPU Control*\n
*===========*

Compared to memory and IO, CPU control is relatively straightforward. If a
workload doesn't get sufficient CPU cycles, it can't perform its job. CPU
usage is primarily measured in wallclock time. The cgroup CPU controller can
distribute CPU cycles proportionally with `cpu.weight`, or limit absolute
consumption with `cpu.max`. In most cases, configuring `cpu.weight` in
higher level cgroups is sufficient.

A number of additional details and variables play a role though. While
wallclock time captures utilization to a reasonable degree, CPU time is an
aggregate measurement encompassing on-CPU compute and cache resources,
memory bandwidth, and more, each of which has its own performance
characteristics.

As CPUs get close to saturation, all the CPU’s subsystems get more bogged
down, and the increase in total amount of work done significantly lags
behind the increase in CPU time. Further muddying the picture, many of the
components are shared across CPU cores and logical threads (hyperthreading),
and how they're distributed by the CPU impacts resource distribution. This
has implications on sideloading, which we'll discuss later.

`cpu.weight` currently repeats scheduling per each level of the cgroup tree.
For scheduling-intensive workloads, this overhead can add up to a noticeable
amount as the nesting level grows. Unfortunately, the only solution
currently is limiting the level at which the CPU controller is enabled.
systemd's "DisableControllers" option can be useful for this purpose.


___*`cpu.max` and priority inversions*___

One of the reasons priority inversions aren't crippling problems for Linux
and most other operating systems is that they're usually self-solving. When
a low priority process ends up blocking the whole system, the system soon
runs out of things to do and the blocking process has the whole machine to
finish what it was doing and unblocks others. This effectively works as a
crude innate priority inheritance mechanism, but it only works when the
system doesn't put strict upper limits on parts of the system.

Let's say the same low priority process is under a stingy `cpu.max` limit
and it somehow ends up blocking a big portion of the system, perhaps through
a kernel mutex. While the rest of the system keeps piling up on the mutex
and the system as a whole is going idle, the low priority process can't run
because it doesn't have enough CPU budget.

While future kernels may improve handling of this particular situation, it's
become a repeating theme in resource control: The more strictly resource
utilization is capped, the more likely priority inversions and system-wide
hangs become. Work-conserving resource control mechanisms are easier to use,
more forgiving in terms of configuration accuracy, and way safer, because
they don't reduce the total amount of work the system does, and thus retain
most of the benefits of the innate priority inheritance behavior.

Unless absolutely necessary, stick with `cpu.weight`. When you have to use
`cpu.max`, avoid limiting it too harshly to avoid system-wide hangs.


___*What about cpuset?*___

cpuset can be useful in some circumstances, but it's limited in control
granularity, often requiring manual configuration, and shares many of the
problems with `cpu.max`. As the number of CPUs on which a workload can run
gets further restricted, priority inversions become more likely, often
causing system-level events that impact the latency profile and decrease
utilization.

While `cpuset` is useful for further optimization, `cpu.weight` is better
suited as a generic system-wide CPU control mechanism, and is the primary
focus in this demo.


___*Let's see it working*___

rd-hashd should already be running at full load. Once hashd is warmed up,
let's disable CPU control, and start a Linux build job with concurrency of
twice the CPU thread count - which is high but not outrageous:

%% (                         	: [ Disable CPU control and start building kernel ]
%% off cpu-resctl
%% on sysload compile-job build-linux-2x
%% )

As the compile jobs ramp up, RPS gets snuffed to zero. Let's stop the
compile job and turn CPU protection back on:

%% (                         	: [ Stop the compile job and restore CPU control ]
%% reset secondaries
%% reset protections
%% )

Wait for the sysload count to drop to zero and rd-hashd to stabilize, then
launch the same compile job again:

%% (                         	: [ Start the compile job ]
%% on sysload compile-job build-linux-2x
%% )

Note that RPS recovers but is still noticeably lower than ~90%, which is
where it should be given the 10:1 cpu weight ratio. This is caused by
scheduler latencies and the muddiness described above around CPU time. We'll
revisit this later when discussing sideloading.


___*Read on*___

Understanding which resources are under contention is critical for resource
control. While cgroup provides resource utilization monitoring, it's
impossible to understand resource shortages from utilization information.
PSI provides critical insight into resource contention, at both the system
level, and per-cgroup.

%% jump comp.psi                 : [ Next: Monitoring Resource Contention with PSI ]

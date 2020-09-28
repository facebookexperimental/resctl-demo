## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.cgroup.cpu: CPU Control
%% reset prep
%% knob sys-cpu-ratio 0.01
%% knob hashd-lat-target 1.0
%% knob hashd-load 0.90
%% on hashd
$$ reset all-with-params

*CPU Control*\n
*===========*

Compared to memory and IO, CPU control is conceptually more straightforward.
If a workload doesn't get sufficient CPU cycles, it can't perform its job.
CPU usage is primarily measured in wallclock time. The cgroup CPU controller
can distribute CPU cycles proportionally with `cpu.weight`, or limit
absolute consumption with `cpu.max`. In most cases, configuring `cpu.weight`
in higher level cgroups is sufficient.

A number of additional details and variables complicate the picture though,
especially for latency-sensitive workloads. As the CPUs get saturated, the
artifacts from time-sharing become more pronounced. When a thread wakes up
to service a request, an idle CPU might not be available immediately and the
scheduling and load balancing decisions start to have significant impacts on
the latency.

Furthermore, while wallclock time captures utilization to a reasonable
degree, CPU time is an aggregate measurement encompassing on-CPU compute and
cache resources, memory bandwidth, and more, each of which has its own
performance characteristics.

As CPUs get close to saturation, all the CPU’s subsystems get more bogged
down, and the increase in total amount of work done significantly lags
behind the increase in CPU time. Further muddying the picture, many of the
components are shared across CPU cores and logical threads (hyperthreading),
and how they're distributed by the CPU impacts resource distribution.

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

Due to the scheduling artifacts and CPU subsystem saturation described
above, the CPU controller usually can't protect a latency-sensitive workload
by itself. While the total CPU cycles are distributed according to the
configured weights, when the CPUs are saturated, the latency increase is
enough to smother a latency-sensitive workload regardless of how low the
priority of the competition may be.

The RPS behavior under CPU competition turned out to be fairly variable
depending on the hardware and kernel configurations, so let's instead watch
how effectively the CPU controller can protect the latency.

rd-hashd is running targeting 90% load and the latency target has been
relaxed from 100ms to 1s - we're asking rd-hashd to meet 90% load regardless
of how much latency deteriorates. Also, ___system___'s CPU weight is reduced
to 1/100th of ___workload___ to make the experiment clearer.

Once hashd is warmed up and the latency is stable below 100ms, let's start a
CPU hog which keeps calculating sqrt() with concurrency of twice the number
of CPU threads.

%% (                         	: [ Start a CPU hog ]
%% on cpu-resctl
%% on sysload cpu-hog burn-cpus-2x
%% )

rd-hashd should be maintaining 90% load level with significantly raised
latency. The CPU hog is running with only 1/100th of the weight but the CPU
controller can't adequately protect rd-hashd's latency.

That's not to say that CPU control isn't effective. Let's turn off CPU
control and see what happens:

%% off cpu-resctl                : [ Turn off CPU control ]

Without CPU control, the overall behavior is clearly and significantly
worse. rd-hashd might even be failing to hold the target load level because
latency keeps climibing above 1s.

The fact the CPU control can't protect latency-sensitive workloads has
implications on sideloading, which we'll discuss later.


___*Read on*___

Understanding which resources are under contention is critical for resource
control. While cgroup provides resource utilization monitoring, it's
impossible to understand resource shortages from utilization information.
PSI provides critical insight into resource contention, at both the system
level, and per-cgroup.

%% jump comp.psi                 : [ Next: Monitoring Resource Contention with PSI ]

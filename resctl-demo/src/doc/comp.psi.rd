## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.psi: PSI - Monitoring Resource Contention with PSI
%% reset secondaries
%% reset protections
%% knob hashd-load 1.0
%% on hashd
$$ reset hashd-params
$$ reset resctl-params
$$ reset secondaries

*PSI - Monitoring Resource Contention with PSI*\n
*=============================================*

When a workload is visibly struggling barely making any progress, we can
tell something is wrong with it. If the IO device is saturated, maybe the
workload is IO bottlenecked or maybe it's thrashing on memory. We can delve
into memory management, IO and other statistics to tell apart different
cases. However, it is often tricky. Maybe only one statistic is clearly
spiking and we can draw a conclusion from that but what if multiple
indications are presented? How would we distinguish how much different
causes are contributing?

Furthermore, if a workload is experiencing moderate resource contentions
where it runs mostly okay but not at the maximum capacity, telling whether
and how much slower the workload is running for what reasons becomes
challenging especially when there are other workloads sharing the system.

PSI (Pressure Stall Information) is a way to measure resource pressure which
indicates how much the workload has been slowed down due to lack of
different resources. For example, memory pressure of 20% indicates that the
workload could have run 20% faster had it access to more memory.

Resource pressure is defined for all three major local resources - CPU,
memory and IO - and measured system-wide and per-cgroup, available in
`/proc/pressure/{cpu, memory, io}` and `/sys/fs/cgroup/CGROUP/{cpu, memory,
io}.pressure` respectively.

There are two types of resource pressure - full and some. Full measures the
duration of time where available CPU bandwidth couldn't be consumed because
all available threads were blocked on the resource and thus full pressure
indicates computation bandwidth loss. Some measures the duration of time
where at least some threads were blocked on the resource. A fully blocked
scope is always some blocked. Some indicates latency impact. For CPU, full
pressure is not defined as full pressure is defined as loss of CPU
bandwidth.

In the top right pane, the columns - "cpuP%", "memP%" and "ioP%" - show,
respectively, CPU some pressure, memory full pressure, and IO full pressure.
In the graph view ('g'), there are graphs for both full and some pressures.


___*PSI in action*___

Let's see how PSI metrics actually behave. rd-hashd is running at the full
load. Wait for the memory usage to stop climbing. It should be showing zero
or very low level of pressure for all three resources. Slowly increase
rd-hashd's memory footprint using the following slider.

%% knob hashd-mem                : Memory footprint :

Soon, RPS will start falilng and both memory and IO pressures going up. Set
the memory footprint so that the workload is suffering but still serving a
meaningful level of load.

Switch to graph view ('g') and take a look at the RPS and pressure graphs.
Notice how the RPS drops match memory and IO pressure spikes. If you add RPS
% and pressure %, it won't stray too far from 100%. We increased the memory
footprint to the point where the workload no longer fits in the available
memory and starts thrashing. It can no longer fully utilize CPU cycles
because it needs to wait for memory too often too long. CPU cycles lost this
way are accounted as memory pressure. As that's the only way we're losing
capacity in this case, the load level and memory pressure will roughly add
up to 100%.

Note that IO pressure is moving together with memory pressure in general but
sometimes registers lower than memory. When a system is short on memory, the
kernel needs to keep scanning memory evicting cold pages and bringing back
pages needed to make forward progress from the filesystems and swap. Waiting
for the pages to be read back from the IO device will take up the lion's
share in lost time, so the two pressure numbers will move in tandem if the
only source of slow down is memory thrashing.

When would IO pressure go up but not memory? - When a workload is slowed
down waiting on IOs and giving it more memory wouldn't reduce the amount of
IOs. This happens when rd-hashd starts. Most memory in the system is idle
and available but rd-hashd needs to load files to build the hot working-set.
Giving it more memory wouldn't speed it up one bit. It still needs to load
what it needs. Let's see how this behaves.

Let's first stop rd-hashd.

%% off hashd                     : [ Stop rd-hashd ]

Give the system some seconds to catch a breath and then start rd-hashd
targeting full load with page cache proportion increased (so that it needs
to load more from files).

%% (                             : [ Start rd-hashd w/ higher page cache portion ]
%% reset hashd-params
%% knob hashd-load 1.0
%% knob hashd-file 1.0
%% on hashd
%% )

Watch how only IO pressure spikes as RPS ramps up and then comes down
gradually as it reaches full load. It starts with cold cache and is mostly
bottlenecked on IO device. As the hot working set gets established, it no
longer is held back by IO and IO pressure dissipates.


___*Full and Some*___

Let's restore default rd-hashd parameters and let it stabilize at the full
load.

%% (                             : [ Reset rd-hashd parameters ]
%% reset hashd-params
%% knob hashd-load 1.0
%% on hashd
%% )

There is some variance in benchmark results. If rd-hashd is staying at 100%
load in the workload row of the top-left pane without any reads being
reported on workload on the top-right pane, slowly push up the following to
take away memory from it until load level falls around 90%.

%% knob   balloon                : Memory balloon :

Once it becomes stable, open graph view ('g') and take a look at the some
CPU pressure and the full and some memory and IO pressure graphs. Full
memory and IO pressures should be very close to zero while some pressures
are raised. The workload is functioning close to its full capacity but with
raised latency - from ~60ms to ~100ms. Most of the latency increase is from
CPU competition but some are from memory competition as indicated by the
three some pressure graphs.


___*Read on*___

Now that we have basic understanding of resoure presusre, let's check out
one of its important use cases - oomd.

%% jump comp.oomd                : [ Next: oomd - The Out-Of-Memory Daemon ]
%% jump comp.cgroup.cpu          : [ Back: CPU Control ]
%% jump index                    : [ Exit: Index ]

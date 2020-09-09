## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.psi: PSI - Monitoring Resource Contention with PSI
%% reset secondaries
%% reset protections
%% off hashd
$$ reset hashd-params
$$ reset resctl-params
$$ reset secondaries

*PSI - Monitoring Resource Contention with PSI*\n
*=============================================*

When a workload is struggling and barely making any progress, we know
something's wrong with it, but exactly what's wrong can be tricky to
determine. If the IO device is saturated, maybe the workload is IO
bottlenecked, or maybe it's thrashing on memory. We can delve into memory
management, IO, and other statistics to analyze different cases. Sometimes
only one statistic is clearly spiking and we can draw a conclusion from
that, but what if there are multiple indicators? How would we determine the
degree to which different causes are contributing?

This is even more challenging for workloads experiencing moderate resource
contention - where they run mostly okay but not at maximum capacity.
Determining whether the workload is running slower, by how much, and for
what reasons, becomes challenging, especially when other workloads are
sharing the system.

PSI (Pressure Stall Information) is a way to measure resource pressure,
which is a measure of how much the workload has been slowed down due to lack
of a given resource. For example, memory pressure of 20% indicates that the
workload could have run 20% faster if it had access to more memory.

Resource pressure is defined for all three major local resources - CPU,
memory, and IO. It's measured system-wide and per-cgroup, and available in
`/proc/pressure/{cpu, memory, io}` and `/sys/fs/cgroup/CGROUP/{cpu, memory,
io}.pressure` respectively.

There are two types of resource pressure - full and some. Full measures the
duration of time during which available CPU bandwidth couldn't be consumed
because all available threads were blocked on the resource - thus, full
pressure indicates computation bandwidth loss. Some measures the duration of
time during which at least some threads were blocked on the resource. A
fully blocked scope is always some blocked. Some indicates latency impact.
For CPU, full pressure is not defined, because full pressure would be
defined as loss of CPU bandwidth which can't be caused by CPU contention.

In the top right pane, the columns - "cpuP%", "memP%" and "ioP%" - show,
respectively, CPU some pressure, memory full pressure, and IO full pressure.
In the graph view ('g'), there are graphs for both full and some pressure.


___*PSI in action*___

Let's see how PSI metrics actually behave. rd-hashd is running at full load.
Wait for the memory usage to stop climbing. It should be showing zero or a
very low level of pressure for all three resources. Slowly increase
rd-hashd's memory footprint using the following slider:

%% knob hashd-mem                : Memory footprint :

Soon, RPS starts falling, and both memory and IO pressure go up. Set the
memory footprint so the workload is suffering, but still serving a
meaningful level of load.

Switch to graph view ('g') and take a look at the RPS and pressure graphs.
Notice how the RPS drops match memory and IO pressure spikes. If you add RPS
% and pressure %, it won't stray too far from 100%. We increased the memory
footprint to the point where the workload no longer fits in the available
memory and starts thrashing. It can no longer fully utilize CPU cycles
because it needs to wait for memory too often and for too long. CPU cycles
lost this way are counted as memory pressure. Since that's the only way
we're losing capacity in this case, the load level and memory pressure will
roughly add up to 100%.

Note that IO pressure moves together with memory pressure in general, but
sometimes registers lower than memory. When a system is short on memory, the
kernel needs to keep scanning memory-evicting cold pages, and bringing back
pages needed to make forward progress from the filesystems and swap. Waiting
for the pages to be read back from the IO device takes up the lion's share
of lost time, so the two pressure numbers move in tandem if the only source
of slowdown is memory thrashing.

When would IO pressure go up, but not memory? This occurs when a workload is
slowed down waiting for IOs, but providing it with more memory wouldn't
reduce the amount of IOs. This is what happens when rd-hashd starts. Most
memory in the system is idle and available, but rd-hashd needs to load files
to build the hot working-set. Giving it more memory wouldn't speed it up one
bit. It still needs to load what it needs. Let's see how this behaves.

Start rd-hashd, targeting full load with page cache proportion increased, so
that it needs to load more from files:

%% (                             : [ Start rd-hashd w/ higher page cache portion ]
%% reset hashd-params
%% knob hashd-load 1.0
%% knob hashd-file 1.0
%% on hashd
%% )

Watch how only IO pressure spikes as RPS ramps up, and then comes down
gradually as it reaches full load. It starts with cold cache and is mostly
bottlenecked on the IO device. As the hot working set gets established, it's
no longer held back by IO, and IO pressure dissipates.


___*Full and Some*___

Let's restore the default rd-hashd parameters, and let it stabilize at full
load:

%% (                             : [ Reset rd-hashd parameters ]
%% reset hashd-params
%% knob hashd-load 1.0
%% on hashd
%% )

There's some variance in benchmark results. If rd-hashd stays at 100% load
in the workload row of the top-left pane, without any reads reported on the
workload in the top-right pane, slowly push up the following knob to take
memory away from it, until load level falls to around 90%:

%% knob   balloon                : Memory balloon :

Once it stabilizes, open graph view ('g') and look at the some stat for CPU
pressure, and the full and some stats for memory and IO, in their respective
pressure graphs. Full memory and IO pressure should be very close to zero,
while some pressures are raised. The workload is functioning close to its
full capacity, but with raised latency - from ~60ms to ~100ms. Most of the
latency increase is from CPU competition, but memory competition also
accounts for part, as indicated by the three pressure graphs for some.


___*Read on*___

Now that you have a basic understanding of resource pressure, let's check out
one of its important use cases - oomd.

%% jump comp.oomd                : [ Next: oomd - The Out-Of-Memory Daemon ]
%% jump index                    : [ Exit: Index ]

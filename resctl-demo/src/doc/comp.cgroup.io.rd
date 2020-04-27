## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.cgroup.io: IO Control
%% reset secondaries
%% reset protections

*IO Control*\n
*==========*

___*Why IO control?*___

Controlling who gets how much IO capacity is critical in achieving
comprehensive resource control. The dependency is obvious for workloads
which perform filesystem or raw IOs directly but because memory management
and IOs are intertwined any workload can become IO dependent especially when
memory becomes short.

All binaries and libraries live on filesystems and they're loaded on demand
and managed dynamically. If an application starts executing a new code path,
it'll wait for reads from the underlying device. If there is even a minute
level of memory competition, pages will be reclaimed and faulted back in as
a part of normal memory management. If IOs from lower priority cgroups are
allowed to saturate the IO device, higher priority cgroups will stall on the
such reads and page faults.

There are many other ways this can happen. Another example is shared
filesystem metadata writes. Most filesystems have operations which are
shared and may be waited by all writers such as journals and transaction
commits. If a cgroup is allowed to flood the device to the level that such
shared metadata IOs are slowed down significantly, everything on the system
which is trying to do any kind of filesystem update, no matter how
miniscule, will get stalled regardless of their relative priorities.

When writes are stalled system-wide like the above, it's not too uncommon
for the system to expose subtle indirect dependency chains which can impact
applications which don't seem to depend on IOs on the surface.

If IO is not controlled, resource protection breaks down no matter how well
memory and other resources are protected.


___*The iocost controller*___

One challenge of controlling IO resources is the lack of trivially
observable cost metric. This is distinguished from CPU and memory where
wallclock time and the number of bytes can serve as accurate enough
approximations.

Bandwidth and iops are the most commonly used metrics for IO devices but
depending on the type and specifics of the device, different IO patterns
easily lead to multiple orders of magnitude variations rendering them
useless for the purpose of IO capacity distribution.  While on-device
time, with a lot of clutches, could serve as a useful approximation for
non-queued rotational devices, this is no longer viable with modern
devices, even the rotational ones.

While there is no cost metric we can trivially observe, it isn't a complete
mystery. For example, on a rotational device, seek cost dominates while a
contiguous transfer contributes a smaller amount proportional to the size.
If we can characterize at least the relative costs of these different types
of IOs, it should be possible to implement a reasonable work-conserving
proportional IO resource distribution.

The iocost controller solves this problem by employing an IO cost model to
estimate the cost of each IO. Each IO is classified as sequential or random
and given a base cost accordingly. On top of that, a size cost proportional
to the length of the IO is added. While simple, this model captures the
operational characteristics of a wide varienty of devices well enough.


___*Owning the queue*___

When a series of writes are issued, many SSDs queue and complete them a lot
faster than they can sustain and then dramatically slow down afterwards. On
some devices, median read completion latency can climb to multiple seconds
only after sustaining some tens of megabytes of writes for a minute.

While this bursty behavior may look good on microbenchmarks, it doesn't buy
us anything of value. The system as a whole can do an a lot better job at
buffering writes anyway and no latency sensitive workload can maintain a
semblance of responsiveness and latency consistency running on a storage
device which takes over hundreds of milliseconds for a read request.

A lot of NVME devices aren't more performant than even mid-range SATA
devices while having many times deeper command queues. The deep queue
exacerbates issues caused by write queueing and allows read commands to
suffer similar issues.

Similar to network Quality-of-Service, you can't implement latency QoS
without owning the queue. If we want to control IO latencies, our control
point must be the choking point in the flow of IOs so that throttling it up
and down has direct latency impacts rather than just changing how much is
bloked on the hardware queue.

Through the cost model, the iocost controller has an understanding of how
much IO the device can do per second and can accordingly pace and limit the
total IO rate. By configuring the maximum IO rate below the measured maximum
with a sufficient buffer, iocost can prevent the device from developing deep
queues and regulate responsiveness.


___*Read on*___

For details on the model and QoS parameters or if you came from the
benchmark page and want to go back, please follow the next link.

%% jump intro.iocost             : [ Iocost Parameters and Benchmark ]
%%
##%% jump comp.cgroup.cpu        : [ Next: CPU Control ]
%% jump comp.cgroup              : [ Up: cgroup and Resource Protection ]
%%
%% jump index                    : [ Exit: Index ]

## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.cgroup.io: IO Control
%% reset secondaries
%% reset protections
%% knob hashd-load 1.0
%% on hashd

*IO Control*\n
*==========*

Controlling the distribution of IO capacity is critical in achieving
comprehensive resource control. This dependency is obvious for workloads
that perform filesystem or raw IOs directly, but because memory management
and IOs are intertwined, any workload can become IO-dependent, especially
when memory becomes short.

All binaries and libraries live on filesystems, and they're loaded on demand
and managed dynamically. If an application starts executing a new code path,
it'll wait for reads from the underlying device. If there's even a minute
level of memory competition, pages are reclaimed and faulted back in as a
part of normal memory management. If IOs from lower priority cgroups are
allowed to saturate the IO device, higher priority cgroups will stall on
such reads and page faults.

There are many other ways this can happen. Another example is shared
filesystem metadata writes. Most filesystems have shared operations, such as
journals and transaction commits, that multiple entities in the system may
need to wait for. If a cgroup is allowed to flood the device to the point
where these shared metadata IOs are slowed down significantly, everything on
the system trying to do any kind of filesystem update will get stalled, no
matter how small, and regardless of their relative priorities.

When writes are stalled system-wide as above, it's not uncommon for the
system to expose subtle indirect dependency chains that can impact
applications which don't seem at first glance to depend on IOs.

If IO is not controlled, resource protection breaks down, no matter how well
memory and other resources are protected.


___*The iocost controller*___

One challenge in IO resource control is the lack of a trivially observable
cost metric, in contrast to CPU and memory, where wallclock time and the
number of bytes serve as sufficiently accurate approximations.

Bandwidth and iops are the most commonly used metrics for IO devices, but
different devices and different IO patterns can easily lead to variations of
multiple orders of magnitude, rendering them useless for IO capacity
distribution. While on-device time, with a lot of clutches, could serve as a
useful approximation for non-queued rotational devices, this is no longer
viable with modern devices, even the rotational ones.

While there's no cost metric we can trivially observe, it isn't a complete
mystery. For example, on a rotational device, seek cost dominates, while a
contiguous transfer contributes a smaller amount proportional to the size.
If we can characterize at least the relative costs of these different types
of IOs, it should be possible to implement a reasonable work-conserving
proportional IO resource distribution.

The iocost controller solves this problem by employing an IO cost model to
estimate the cost of each IO. Each IO is classified as sequential or random
and given a base cost accordingly. On top of that, a size cost proportional
to the length of the IO is added. While simple, this model captures the
operational characteristics of a wide variety of devices well enough.


___*Owning the queue*___

When a series of writes are issued, many SSDs queue and complete them a lot
faster than they can sustain, and then dramatically slow down afterward. On
some devices, median read completion latency can climb to multiple seconds
only after sustaining some tens of megabytes of writes for a minute.

While this bursty behavior may look good on microbenchmarks, it doesn't buy
us anything of value. The system as a whole can do a much better job at
buffering writes anyway, and no latency-sensitive workload can maintain a
semblance of responsiveness and latency consistency running on a storage
device that takes over hundreds of milliseconds for a read request.

A lot of NVME devices aren't more performant than even mid-range SATA
devices, while having many times deeper command queues. The deep queue
exacerbates issues caused by write queueing and allows read commands to
suffer similar issues.

Similar to network Quality-of-Service, you can't implement latency QoS
without owning the queue. If we want to control IO latencies, our control
point must be the choking point in the flow of IOs so that throttling it up
and down has direct latency impacts, rather than just changing how much is
blocked on the hardware queue.

Through the cost model, the iocost controller has an understanding of how
much IO the device can do per second, and can pace and limit the total IO
rate accordingly.


___*IO control and filesystem*___

When a file is read or written, it has to run through the tall stack of
memory management, filesystem and IO layer before hitting the storage
device. If any part of the stack has priority inversions where lower
priority cgroups can block higher priority ones, IO control, and thus
resource control, break down.

Modern filesystems often have intricate and complex interlockings among
operations, making straightforward issuer-based control insufficient. For
example, if we blindly throttle down metadata updates from a low priority
cgroup, other metadata updates from higher priority cgroups might get
blocked behind due to constraints internal to the filesystem.

To avoid such conditions, a filesystem has to be updated to judiciously
apply different control strategies depending on the situation. Data writes
without further dependency can be controlled directly. Metadata updates
which may cause priority inversions must be performed right away, regardless
of who caused them. However, we want to balance the book after the fact by
back-charging the issuing cgroup, so the book can be balanced over time.

Currently, the only modern filesystem that has full support for IO control
is btrfs. ext2 without journaling works too, but likely isn't adequate for
most applications at this point. The impact of these priority inversions
will vary depending on how much of a bottleneck IO is on your system.


___*Let's put it through the paces*___

rd-hashd should already be running at full load. Once it warms up, let's
disable IO control and start an IO bomb, that causes a lot of filesystem
operations. (Memory bomb being used for now)

%% (                             : [ Disable IO control and start IO bomb ]
%% off io-resctl
%% on sysload memory-bomb memory-growth-1x
%% )

The level of impact depends on your IO device but it will be impacted. Once
it's struggling, let's turn IO protection back on and see how it behaves:

%% (                             : [ Restore IO control ]
%% reset protections
%% )

The kernel is able to protect hashd indefinitely. oomd's system.slice
long-term thrashing policy might trigger and kill the IO bomb though.

If you're curious, set up a system with a different filesystem, repeat this
test and see how it works.


___*Read on*___

For details on the model and QoS parameters, or if you came from the
benchmark page and want to go back, follow the next link.

%% jump intro.iocost             : [ Iocost Parameters and Benchmark ]
%%
%% jump comp.cgroup.cpu          : [ Next: CPU Control ]
%% jump index                    : [ Exit: Index ]

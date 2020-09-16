## Copyright (c) Facebook, Inc. and its affiliates.
%% id senpai.senpai: Senpai
%% reset prep
%% knob hashd-load 0.25
%% on hashd

*Senpai*\n
*======*

Senpai is a tool that uses PSI memory pressure metrics to maintain a
moderate level of reclaim activity, keeping memory management informed with
hot working set knowledge, without impacting performance.

***WARNING***: Senpai does not work on rotating hard disks.

Before we get into the details, let's kick off Senpai.

If you've come from the previous page, rd-hashd should be running at 25%
load, with its memory usage filling most of the system. If so, skip the
following two steps. Otherwise, ramp it up to 100%, wait until it fills up
the memory, and then reduce it back to 25% using the following buttons:

%% knob hashd-load 1.0           : [ Set rd-hashd load level to 100% ]
%% knob hashd-load 0.25          : [ Set rd-hashd load level to 25% ]

Once rd-hashd is running at 25% load with high memory usage, enable Senpai
on workload.slice:

%% on oomd-work-senpai           : [ Enable Senpai on workload.slice ]

OOMD will start logging a line every second in the "Management logs" pane on
the left. You can also view these logs in the "rd-oomd" entry in the log
view ('l').


___*What is Senpai?*___

The previous page demonstrated the main challenge of memory sizing: Losing
hot working set knowledge due to loss of reclaim activities. Senpai is a
simple mechanism that ensures there's always a light level of reclaim
activity - enough to maintain working set knowledge, but not enough to cause
noticeable impact on the workload's performance.

Senpai achieves this by modulating `memory.high` while monitoring the PSI
memory pressure. It gradually clamps down on the allowed amount of memory,
until signs of memory contention are detected, and then backs off. It
continues monitoring PSI and modulating `memory.high`, keeping the workload
right on the edge, where it has just enough memory to run unimpeded.

Note that this is inherently doing more work than necessary. The system
doesn't need to reclaim memory at all right now, but Senpai is making it go
through all the motions - scanning the pages to discover hot and cold pages,
kicking out colder pages, bringing them back as needed and so on. Because
the rate of reclaim is relatively low, the CPU overhead from scanning pages,
and the IO overhead from faulting some of them back in, tends to be
negligible. When the workload actually saturates the system, PSI levels tell
senpai to back off and go idle, and both the CPU and IO overhead disappear.

Senpai should have shaved off some memory by now. We reduced rd-hashd load
to 25% which reduces its memory footprint by around 37.5% (half of memory
usage doesn't scale with RPS). On a test machine with 32G of memory, memory
usage at the full RPS is around 27G, so I'd expect it to shrink to around
17G. This is a rough estimate - the actual number will depend on benchmark
variance, SSD performance, and Senpai's memory pressure threshold and
configuration.

On a test setup with 32G of memory running and 250G Samsung 860 EVO SSD,
Senpai reduced rd-hashd's memory usage by about 3G, down to 24G in six
minutes. As the size goes down, the speed becomes slower and it converged on
17.5G in around forty minutes. The speed you see on your test setup will
depend on how much memory you begin with, and the performance of the
underlying IO device.

As you can see, the time scale for convergence is in tens of minutes. The
Senpai configuration included for this demo is pretty aggressive too.
Production deployments at Facebook use significantly more conservative
pressure thresholds, with the convergence timescale reaching a few hours
after adjustment periods. While slow, this provides reliable memory usage
information across the fleet at virtually no cost.


___*Future of Senpai*___

Senpai is already in wide-scale production across numerous workloads at
Facebook. Its beauty is in its simplicity: All it does is monitor the PSI
`some` total memory pressure metric of the target cgroup, and adjusts
memory.high up and down accordingly. With just that, it can keep a moderate
level of reclaim going, such that what's being used equals what's needed.

Currently, the convergence rate is limited to the IO device, which doesn't
always yield desirable precision, particularly for quickly changing
workloads, so we're now experimenting with in-memory reclaim caches, such as
zswap and zcache.

___*Read on*___

On the next page, you can experiment with Senpai in different situations.

%% jump senpai.exp               : [ Next: Senpai Playground ]

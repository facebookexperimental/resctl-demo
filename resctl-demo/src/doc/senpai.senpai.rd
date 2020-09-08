## Copyright (c) Facebook, Inc. and its affiliates.
%% id senpai.senpai: Senpai
%% reset secondaries
%% reset protections
%% knob hashd-load 0.25
%% on hashd

*Senpai*\n
*======*

***WARNING: Senpai does not work on rotating hard disks.***

Let's kick off senpai first.

If you've come from the previous page, rd-hashd should be running at 25%
load with its memory usage filling most of the system. If so, skip this
paragraph. Otherwise, ramp it up to 100%, wait it fills up the memory and
then reduce it back to 25% using the following buttons.

%% knob hashd-load 1.0           : [ Set rd-hashd load level to 100% ]
%% knob hashd-load 0.25          : [ Set rd-hashd load level to 25% ]

Once rd-hashd is running at 25% load with high memory usage. Enable senpai
on workload.slice.

%% on oomd-work-senpai           : [ Enable Senpai on workload.slice ]

OOMD will start logging a line every second in the "Management logs" pane on
the left. You acn also view these logs in the "rd-oomd" entry in the log
view ('l').


___*What is Senpai?*___

The previous page illustrated that the main challenge of memory sizing is
losing hot working set knowledge without reclaim activities. Senpai is a
simple mechanism to ensure that there always are a light level of reclaim
activities - enough to maintain working set knowledge but not enough to
cause noticeable impact on the workload performance.

Senpai achieves this by modulating `memory.high` while monitoring the PSI
memory pressure. It gradually clamps down the allowed amount of memory until
signs of memory contention are detected and then backs off. By keeping doing
that, it keeps the workload right on the edge where it has just enough
memory to run unimpeded.

Note that this inherently is doing more work than necessary. The system
doesn't need to reclaim memory at all right now but senpai is making it go
through all the motions - scanning the pages to discover hot and cold pages,
kicking out colder pages, bringing them back as needed and so on. Because
the rate of reclaim is relatively low, the CPU overhead tends to be
negligible. However, IO device usage can be higher, while still at a low to
moderate level, as the system is constantly reclaiming and faulting back in
pages. This extra IO usage goes away as the system gets fully saturated.

Senpai should have shaved off some memory by now. We reduced rd-hashd load
to 25% which reduces its memory footprint by around 37.5% (half of memory
usage doesn't scale with RPS). On my test machine with 32G of memory, the
memory usage at the full RPS is around 27G, so I'm expecting it to shrink to
around 17G. This is a rough estimate and the actual number will depend on
benchmark variance, SSD performance and senpai's memory pressure threshold
and other configurations.

On my test setup with 32G of memory running and 250G Samsung 860 EVO SSD,
senpai reduced rd-hashd's memory usage by about 3G down to 24G in six
minutes. As the size goes down, the speed becomes slower and it converged on
17.5G in around fourty minutes. The speed you see on your test setup will
depend on how much memory you begin with and the performance of the
underlying IO device.

As you can see, the time scale for convergence is in tens of minutes. The
senpai configuration included for this demo is pretty aggressive too.
Production deployments at Facebook use significantly more conservative
pressure thresholds and adjustment periods with convergence timescale
reaching a few hours. While slow, this provides reliable memory usage
information across the fleet at virtually no cost.


___*Future of Senpai*___

Senpai is already in wide-scale production across numerous workloads at
facebook and its beauty is in its simplicity. All it does is monitoring the
some total memory pressure of the target cgroup and adjusting up and down
memory.high accordingly. With just that, it can keep a moderate level of
reclaim going so that what's being used equals what's needed.

However, there are some limitations to this approach. To determine the
working set size, all that's needed is the access pattern learning through
scanning, not the whole reclaim. Using memory.high to induce scanning means
we're dropping and faulting back pages when there is enough physical memory
available to keep them around. This limits senpai in a few ways:

* It can cause regressions for applications which have long periods where
  some part of otherwise hot working set is not used.

* As too aggressive clamping down can slow down the workload, senpai needs
  to be conservative. This limits the speed of convergence.

* It causes extra IOs.

We are working to implement IO-less senpai which excercises scanning without
actual reclaim to address the above shortcomings.


___*Read on*___

On the next page, you can experiment with senpai on different situations.

%% jump senpai.exp               : [ Next: Senpai Playground ]
%% jump index                    : [ Exit: Index ]

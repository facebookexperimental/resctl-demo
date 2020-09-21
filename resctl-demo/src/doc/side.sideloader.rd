## Copyright (c) Facebook, Inc. and its affiliates.
%% id side.sideloader: The Sideloader
%% reset prep
%% knob sys-cpu-ratio 0.01
%% knob sys-io-ratio 0.01
%% knob hashd-load 0.6
%% on hashd

*The Sideloader*\n
*==============*

The naive approach to sideloading we took in the previous section -
prioritizing the main workload using resource controllers - wasn't good
enough. Let's examine why.

___*CPU headroom*___

In the previous chapter's experiments, the resource pressure graphs showed
CPU was the only resource for which rd-hashd experienced contention,
suggesting that most of latency increase is coming from CPU side.

Why is that though? ___system___'s CPU weight was set at 1/100 of
___workload___. Why did 1% weight have such pronounced impact on baseline
latency? Here are two reasons:

1. The kernel scheduler may take some time to start running a newly runnable
   high priority thread due to limitations in preemption and load balancing.
   The level of latency this introduces changes according to kernel
   preemption settings and scheduler parameters.

2. The usual CPU utilization measured in wallclock time is an aggregate
   metric encompassing many sub-resources in and around the CPUs, including
   the actual computation units, registers and cache hierarchy, memory and
   bus bandwidth, and so on. These sub-resources are allocated dynamically
   across competing execution threads, and, as they get close to their
   saturation points, develop contentions that lower the amount of
   processing that can be performed over the same time period. In short,
   CPUs are faster when they have slack in utilization.

While some gains are possible by changing preemption configuration and
tuning scheduler parameters, this doesn't address #2 at all, and gains made
this way are fairly limited in most cases, barely making a dent in reducing
the added latency to an acceptable level.

This is where CPU headroom comes in. If we keep some portion of CPU time
idle when running sideloads, we can address both sources of latency impact:
When a high priority thread becomes runnable, it's more likely to find a CPU
ready to execute it right away, and when the CPU starts executing the high
priority thread, it won't get bogged down by sub-resource contention, since
they all have some slack.


___*The sideloader*___

The sideloader is a prototype userspace sideload management agent that
implements the following:

* CPU headroom, by dynamically adjusting the maximum CPU bandwidth sideloads
  can consume, using `cpu.max` so that the main workload always has a
  configurable level of CPU utilization headroom. For example, if the main
  workload is currently consuming 60% and the headroom is configured at 15%,
  sideloads can use only up to 25% of CPU time.

* Sideload freezing and killing. Depending on the memory footprint and
  access pattern of the main workload, even a low level of memory activity
  from sideload can have an oversized impact on the main workload operating
  at full capacity. To guarantee that sideloads don't get in the way when
  the main workload needs the whole machine, the sideloader can activate the
  cgroup2 freezer to make sideloads completely inert and later kill them if
  end up staying frozen for too long.

Sideloader is already running as part of this demo, as rd-sideloader
service. the "sideload" line in the upper left pane, reports its status:

  [ sideload  ] jobs:  0/ 0  failed:  0  cfg_warn:  0  -overload -crit

* "sideload": Green/red indicates whether sideloader is running or stopped.
  If resource control for any resource is disabled, sideloader will be
  stopped.

* "jobs": The number of active/all sideloads.

* "failed": The number of failed sideloads.

* "cfg_warn": Sideloader periodically performs system configuration sanity
  checks to ensure all resource control configurations are set up to isolate
  primary workloads from sideloads. cfg_warn reports the number of
  configuration errors. You can find the details in rd-sideloader log and
  its status report file - `/var/lib/resctl-demo/sideloader/status.json`.

* "[+|-]overload": Indicates whether the system is overloaded (+) or not
  overloaded (-). When overloaded, all sideloads are frozen and optionally
  killed after the specified timeout.

* "[+|-]crit": Indicates whether the system is overloaded to a critical
  level (+), or not critical (-). When critical, all sideloads will be
  killed.

Now, let's repeat the experiment from the last section, but launch the Linux
compile job as a sideload that's supervised by the sideloader.

rd-hashd should already be running at 60% load. Once it warms up, let's
start a Linux compile job with 2x CPU count concurrency as before. It'll
have the same resource weights as before; the only difference is that it's
now running under the supervision of sideloader:

%% (                             : [ Start linux compile job as a sideload ]
%% on sideload compile-job build-linux-2x
%% )

While the Linux source tree is being untarred, depending on the memory
situation and IO performance, you may see a brief spike in latency as the
kernel tries to figure out the hot working set of the primary workload. Once
the compile jobs start running, ___sideload___ starts consuming CPU time and
pushing up CPU utilization. Look in the upper right pane, and at the
utilization graphs in the graph view ('g'). CPU utilization will go up but
won't reach 100%.

Look at the latency graph. It's gone up a bit, but a lot less than before,
when we ran it as a sysload. You can change the time scale with 't/T' to
compare the latencies before and after. How much sideloads impact latency is
determined by the headroom: The bigger the headroom, the lower the CPU
utilization, and the lower the latency impact.

The current state, in which the sideload fills up the machine's left-over
capacity, with a controlled latency impact on the main workload - can be
sustained indefinitely. Let's see how ramping up to full load behaves:

%% knob hashd-load 1.0           : [ Set full load ]

The difference from the no sideload case should be significantly less
pronounced. Look at how the sideloader state transitions to overload,
freezing the Linux compile job. The build job is configured to expire after
being frozen for 30s. Let's wait and see what happens.

We just demonstrated that, with the help of sideloader, sideloads can
utilize the left-over capacity of the machine, with only a controlled and
configurable latency impact on the main workload, and without significantly
impacting the main workload's ability to ramp up when needed.


___*Read on*___

Now that we saw the basics of sideloading, let's exmaine some of the
details.

%% jump side.details             : [ Next: Some Details on Sideloading ]

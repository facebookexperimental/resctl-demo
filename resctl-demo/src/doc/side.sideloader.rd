## Copyright (c) Facebook, Inc. and its affiliates.
%% id side.sideloader: The Sideloader
%% reset secondaries
%% reset protections
%% knob sys-cpu-ratio 0.01
%% knob sys-io-ratio 0.01
%% knob hashd-load 0.5
%% on hashd
$$ reset resctl-params

*The Sideloader*\n
*==============*

The naive approach to sideloading - prioritizing the main workload using
resource controllers - wasn't good enough. Let's examine why that was.

___*CPU headroom*___

During the experiments in the previous chapter, the resource pressure graphs
were indicating that the only contention that rd-hashd was experiencing was
on CPU, suggesting that most of latency increase is coming from CPU side.

Why is that tho? system.slice's CPU weight was set at 1/100 of
workload.slice. Why did 1% weight have such pronounced impact on baseline
latency? Here are two reasons:

1. The kernel scheduler may take some time to kick out the currently running
   low priority thread when a new higher priority thread becomes runnable.
   The level of latency introduced by this will change according to kernel
   preemption settings and scheduler parameters. While this can be improved
   to some degree, in most server setups, preemption configuration is
   determined by balancing the bandwidth gain against latency impact and
   shifting it for lower latencies can incur noticeable costs.

2. The usual CPU utilization measured in wallclock time is an aggregate
   metric encompassing many sub-resources in and around the CPUs including
   the actual computation units, registers and cache hierarchy, memory and
   bus bandwidth and so on. These sub-resources are allocated dynamically
   across competing execution threads and as they get close to their
   saturation points develop contentions which lower the amount of
   processing which can be performed over the same time period. In short,
   CPUs are faster when they have slack in utilization.

While there may be some gains to be made by changing preemption
configuraiton and tuning scheduler parameters, it doesn't address #2 at all
and the gains which can be made this way are fairly limited in most cases
barely making a dent in reducing the added latency to an acceptable level.

This is where CPU headroom comes in. If we keep some portion of CPU time
idle when running sideloads, it can address both sources of latency impacts
- when a high pririty thread becomes runnable, it's more likely to find a
CPU which is ready to execute it right away and when the CPU starts
executing the high priority thread, it won't get bogged down by sub-resource
contention as they all have some slacks.


___*The sideloader*___

The sideloader is a userspace sideload management agent prototype which
implements the followings:

* CPU headroom by dynamically adjusting the maximum CPU bandwidth that
  sideloads can consume using `cpu.max` so that the main workload always has
  a configurable level of CPU utilization headroom. For example, if the main
  workload is currently consuming 50% and the headroom is configured at 15%,
  sideloads will only be allowed to use upto 25% of CPU time.

* Sideload freezing and killing. Depending on the memory footprint and
  access pattern of the main workload, even a low level of memory activity
  from sideload can have an oversized impact on the main workload operating
  at full capacity. To guarantee that sideloads don't get in the way when
  the main workload needs toeh whole machine, the sideloader can activate
  the cgroup2 freezer to make sideloads completely inert and later kill them
  if end up staying frozen for too long.

Sideloader is already running as rd-sideloader service as a part of this
demo. If you look at the upper left pane, the "sideload" line is reporting
its status.

  [ sideload  ] jobs:  0/ 0  failed:  0  cfg_warn:  0  -overload -crit

* "sideload": Green/red indicates whether sideloader is running or stopped
  respectively. If resource control for any resource is disabled, sideloader
  will be stopped.

* "jobs": The number of active / all sideloads.

* "failed": The number of failed sideloads.

* "cfg_warn": Sideloader periodically performs system configuration sanity
  checks to ensure that all resource control configurations are set up to
  isolate the primary workloads from the sideloads. cfg_warn reports the
  number of configuration errors. You can find out the details in
  rd-sideloader log and its status report file -
  `/var/lib/resctl-demo/sideloader/status.json`.

* "[+|-]overload": Whether the system is overloaded. When overloaded, all
  sideloads are frozen and optionally killed after the specified timeout.
  "+" means overloaded, "-" not.

* "[+|-]crit": Whether the system is overloaded to a critical level. When
  critical, all sideloads will be killed. "+" means critical, "-" not.

Let's repeat the experiment from the last section but launch the linux build
job as a sideload which is supervised by the sideloader.

rd-hashd should already be running at 50% load. Once it warms up, let's
start a linux build job with 2x CPU count concurrency as before. It'll have
the same resource weights as before the only difference is that it's now
being run under the supervision of sideloader.

%% (                             : [ Start linux build job as a sideload ]
%% on sideload build-linux build-linux-2x
%% )

While the linux source tree is being untarred, depending on the memory
situation and IO performance, you may see a brief spike in latency as the
kernel tries to figure out the hot working set of the primary workload. Once
the compile jobs start running, `sideload.slice` will start consuming CPU
time pushing up CPU utilization. Take a look in the upper right pane and
utilization graphs in the graph view 'g'. CPU utilization will go up but
won't reach 100%.

Take a look at the latency graph. It's gone up a bit but a lot less than
compared to before when we ran it as a sysload. You can change the time
scale with 't/T' to compare the latencies before and after. How much
sideloads impact the latency is determined by how big the headroom is. The
bigger the headroom, the lower the CPU utilization and the lower the latency
impact.

The current state - sideload filling up the left-over capacity of the
machine with a controlled latency impact on the main workload - can be
sustained indefinitely. Let's see how ramping up to full load behaves.

%% knob hashd-load 1.0           : [ Set full load ]

The difference from no sideload case should be significantly less
pronounced. Look at how the sideloader state transitions to overload
freezing the build job. The linux build job is configured to expire after
frozen for 30s. Let's wait and see what happens.

We just demonstrated that, with the help of sideloader, sideloads can
utilize the left-over capacity of the machine with only a controlled and
configurable latency impact on the main workload and without significantly
impacting the main workload's ability to ramp up when needed.


___*Read on*___

Now that we saw the basics of sideloading, let's exmaine some of the
details.

%% jump side.details             : [ Next: Some Details on Sideloading ]
%% jump side.intro               : [ Prev: What is Sideloading? ]
%% jump index                    : [ Exit: Index ]

## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.cgroup: Cgroup and Resource Protection
%% reset secondaries
%% reset protections

*Cgroup and Resource Protection*\n
*==============================*

___*Web server and external memory leak*___

Imagine a fleet of web servers. Their main job would be running the web
application to service user requests. However, that's not the only thing
that needs to run. There are a lot of managerial tasks to do - system
software and the application have to be updated regularly, fleet-wide
maintenance tools have to run to update and keep the configuration in
compliance, various monitoring and alarm frameworks and so on.

Let's say one of the maintenance program started by cron is malfunctioning
and keeps leaking memory. Let's say the malfunction is wallclock dependent.
A bad scenario - it would stay hidden during testing and then trigger at the
same time in production. What would happen to the system and fleet? Let's
simulate the scenario with rd-hashd and a program which leaks memory. Let's
first start hashd at full load.

%% (                             : [ Start hashd at full load ]
%% knob hashd-load 1.0
%% on hashd
%% )

Give it some time so that it can ramp up and its memory footprint grows to
closely fill up the machine. Check out the memory utilization graph by
pressing 'g'.

Once the workload is ramped up sufficiently, let's disable all resource
control mechanisms and start a memory bomb program to see how the system
behaves without any protection. The proplematic program will start as
rd-sysload-memory-bomb.service under system.slice.

%% (                             : [ Disable all resource control features and start memory bomb ]
%% off cpu-resctl
%% off mem-resctl
%% off io-resctl
%% off oomd
%% on sysload memory-bomb memory-growth-50pct
%% )

Note that you lose visibility into which cgroup is using how much IO. This
is because per-cgroup IO statistics are tied to the IO controller being
enabled. The graphs aren't gonna be too useful but you can monitor the total
usage on the last row in the statistics pane right above.

**WARNING**: As the system is running without any protection, nothing can
guarantee the system's responsiveness. Everything including this demo
program will get sluggish and may completely stall. Depending on how the
kernel OOM killer reacts, the system may or may not recover in a reasonable
amount of time. If you want to avoid possible need for a reboot, stop the
experiment once the system becomes sluggish.

%% (                             : [ Stop the experiment and restart rd-hashd ]
%% reset all-workloads
%% reset protections
%% on hashd
%% )

That wasn't ideal, was it? If this were production environment and the
failure happened across a large number of machines at the same time, our
users are noticing the disturbances and the whole site might be going down.

How bad the system behaved will depend on the IO device. On SSDs with high
and consistent performance, the system would be able to ride it out longer
and the degradation will be more gradual. On a waker storage device, the
margin is narrower and the deterioriation more abrupt.

This is not because rd-hashd is particularly IO heavy. The only "file" IO it
does is log writes which is calibrated to ~5% of maximum write bandwidth.
The memory bomb doesn't do any file IOs either. All the IOs and thus the
dependence on storage performance is due to the fact that memory management
and IO are intertwined. Your IO device is a critical component in deciding
how big your workload's memory footprint can be and how gracefully the
system can degrade under stress with or without resource control.


___*The cgroup tree*___

That was frustrating to watch. We knew which application was important. We
*knew* that the management portion is less important and more likely to
cause issues with the mountain of scripts running. As the situation was
deterioriating, it was clear as day what should have been throttled.

But the system didn't know. To the kernel, they were all just processes. It
couldn't tell that rd-hashd, standing in for our web server, was the primary
workload and that the memory bomb which was eating memory like a monster was
a malfunction. At least memory and IO are concerned, they were on equal
footing and the kernel tried to balance the two as best as it could as the
system was descending into a thrashing live-lock.

cgroup (control group) is the Linux kernel feature which enables users to
tell the kernel how the system is composed and how resources should be
distributed among the applications.

cgroup has a tree structure just like your filesystem, but a tree node
contains processes rather than files. Such node is called, somewhat
confusingly, a cgroup. This tells the kernel how the processes are grouped
into an appliation and then how applications relate to each other. Along
this hierarchy, cgroup controllers can be configured to monitor resource
consumptions and control distributions.

A slice is systemd term for an intermediate cgroup. In this demo, we're
primarily concerned with the top-level slices, or cgroups. Because cgroup
resource control is hierarchical, if we control how resources flow into each
top-level cgroup and organize applications below them, we can control how
resources are distributed all across the system. The following top-level
slices are in use:

* workload.slice: This is where the primary workloads of the system run. Our
  latency sensitive primary workload - rd-hashd - runs here too as
  workload.slice/rd-hashd-A.service.

* sideload.slice: This is where secondary opportunistic side workloads run.
  We'll revisit sideloads later.

* hostcritical.slice: Some applications are critical for the basic health of
  the system. Here are some examples:

    * A systemd-based system might as well be off if dbus is not responding.

    * There may be a fleet management software whose failure equals the
      machine not being considered online at all.

    * Software like oomd whose job is protecting the system. If they can't
      run, they can't protect the system.

    * The ability to debug is paramount - if we can't log into the machine
      and find out what went wrong, we can't fix problems. Logging and
      logging in are critical. Some monitoring may fall in this category
      too.

    * In this demo, we'll drive the system towards the edge over and over
      and we want this demo to remain functioning and responsive. As such,
      this program itself is a critical monitoring and control software for
      the purpose of this demo and also serves as a demonstration of what
      resource control can do - no matter what happens to the rest of the
      system, this demo will run and respond to you as long as resource
      control protections are enabled.

* system.slice: Everything else. All the management, non-critical monitoring
  and the kitchensink. We want these to run but it's also not the end of the
  world to slow down or even kill and restart if the situation gets really
  dire.

* init.scope: systemd lives here, which is critical for the whole system. We
  set up resource protections for this cgroup similar to hostcritical.slice.
  For brevity, we won't discuss this cgroup further in this demo. All
  discussions which apply to hostcritical.slice apply to this cgroup.

* user.slice: This is where user's login sessions live. In some server
  deployments, this isn't used at all. In desktop setups, configuring so
  that each session has some protection and no one application suffocate
  everyone else would be useful. This demo sets up some basic protections
  for user.slice in case this demo is being accessed from local session but
  detailed discussion of desktop use case is out of scope for this demo.
  user.slice protection won't be discussed further in this demo.

If you look above, the upper right pane shows various statistics for each
top-level cgroup. The last line tagged with "-" is system total. Many graphs
in the graph view ('g'), plot the same metric for workload, sideload and
system slices.

You can check out what cgroups are on your system by running `cgls` and the
kernel interface is a pseudo filesystem mounted under /sys/fs/cgroup.


___*Resource protection*___

Now that the kernel understands what applications are on the system and how
they're grouped, it can monitor resource consumptions and control
distributions along that hierarchy. Let's repeat the same test run we did
above but without turning off all the resource protection configurations.

rd-hashd should already be running at full tilt and all warmed up now. Let's
start the same memory bomb but not touch anything else.

%% on sysload memory-bomb memory-growth-50pct : [ Start memory bomb ]

Monitor the RPS and other resource consumptions in the graph view ('g'). The
RPS may go down a little bit and dip occassionally but it'll stay close to
the full capacity no matter how long you let the memory bomb go on and
eventually the memory bomb will be killed off by oomd. Try it multiple times
by clearing the memory bomb with the following button and restarting it with
the above button.

%% off sysload memory-bomb       : [ Stop memory bomb ]

Our site is not going down and our users probably aren't even noticing. If
we have a working monitoring mechanism, the oomd kills will raise an alarm
and people will soon find out which program was malfunctioning and hunt down
the bug. What could have been a site-wide outage got de-escalated into an
internal oops.

This is cgroup resource protection at work. The kernel understood which part
was important and protected its resources at the expense of the memory bomb
running in the low priority portion of the system. Eventually, the situation
for the memory bomb became unsustainable and oomd terminated it.

We'll go into details in the following pages but here's a brief summary of
how resource protection is configured in this demo.

* Memory

  * workload.slice: memory.low - the best effort memory protection - is set
    up so that most of the system's memory is available to the primary
    workloads if they want it.

  * hostcritical.slice: memory.min - the more strict version of memory.low -
    is set up so that 512MB is always available to hostcritical
    applications.

  * sideload.slice: memory.high is configured so that sideload doesn't
    expand to more than half of all memory. This isn't strictly necessary.
    Its only role is limiting the maximum size sideloads can expand to when
    the system is idle otherwise, so that primary workload doesn't have to
    wait for it to be kicked out while ramping up.

* IO: All configurations are through the io.cost controller and thus only
  have one value to configure - the weight.

    workload : hostcritical : system : sideload = 500 : 100 : 50 : 1

  We want the majority to go to the workload. hostcritical shouldn't need a
  lot of IO bandwidth but can still get a fair bit when needed. system can
  get one tenth of workload at maximum, which isn't huge but still enough to
  make non-glacial forward progress. sideload only gets what's left over.

  Note that the weight based controllers such as io.cost are
  work-conserving. The ratio is enforced only when the underlying resource
  is contended. If the main workload is hitting the disk hard and a system
  service wants to use it at the same time, the system services would only
  be able to get upto 1/10 of what workload gets, but if the disk is not
  contended, system can use however much is available.

* CPU: All configurations are through cpu.weight.

    workload : hostcritical : system : sideload = 100 : 10 : 10 : 1

  The way cpu.weight works and is configured is very similar with IO.


___*Read on*___

For more details on cgroup:

 * Maximizing Resource Utilization with cgroup2
   https://facebookmicrosites.github.io/cgroup2/docs/overview

 * Control Cgroup v2
   https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/Documentation/admin-guide/cgroup-v2.rst

Now that we have the basic understanding of cgroup and resource protection,
let's delve into details of how different parts of cgroup work.

%% jump comp.cgroup.memory       : * Memory Control
%% jump comp.cgroup.io           : * IO Control
%% jump comp.cgroup.cpu          : * CPU Control
%%
%% jump comp.cgroup.memory       : [ Next: Memory Control ]
%% jump index                    : [ Exit: Index ]

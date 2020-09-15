## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.cgroup: Cgroup and Resource Protection
%% reset secondaries
%% reset protections
%% knob hashd-load 1.0
%% on hashd

*Cgroup and Resource Protection*\n
*==============================*

___*Scenario: A web server, an external memory leak, and no resource control*___

Imagine a fleet of web servers whose main job is running a web application
to service user requests. But like all systems, a lot of other apps need to
run: The system software and the web app require regular updates; fleet-wide
maintenance tools keep the configuration in compliance; various monitoring
and alarm frameworks need to run - and so on.

Let's say a maintenance program started by cron is malfunctioning and keeps
leaking memory, and that the malfunction is wall-clock dependent: This is a
bad scenario, that would stay hidden during testing, and then trigger at the
same time in production. What would happen to the system and fleet without
resource control enabled?

We can simulate this scenario with the rd-hashd workload simulator, and a
test program intentionally designed to leak memory. rd-hashd is already
started targeting the full load. Give it some time to ramp up, and let its
memory footprint grow to closely fill up the machine. Check out the memory
utilization graph by pressing 'g'.

Once the workload sufficiently ramps up, you can see how the system behaves
without any protection, by selecting the button below to disable all
resource control mechanisms and start a memory-hog program. The problematic
program will start as rd-sysload-memory-hog.service under system.slice:

%% (                             : [ Disable all resource control features and start memory hog ]
%% off cpu-resctl
%% off mem-resctl
%% off io-resctl
%% off oomd
%% on sysload memory-hog memory-growth-1x
%% )

***WARNING***: Because the system is running without any protection, nothing
can guarantee the system's responsiveness. Everything, including this demo
program, will get sluggish, and might completely stall. Depending on how the
kernel OOM killer reacts, the system may or may not recover in a reasonable
amount of time. To avoid the need for a reboot, stop the experiment once the
system becomes sluggish:

%% (                             : [ Stop the memory hog and restore resource control ]
%% reset secondaries
%% reset protections
%% )

That wasn't ideal, was it? If this were a production environment and the
failure happened across a large number of machines at the same time, our
users would notice the disturbances, and the whole site might go down.

How bad the system behaves in this case depends on the IO device. On SSDs
with high and consistent performance, the system could ride it out longer,
and the degradation would be more gradual. On less performant storage
devices, the margin is narrower and the deterioration more abrupt.

This isn't because rd-hashd is particularly IO heavy: The only "file" IO it
does are log writes, calibrated to ~5% of maximum write bandwidth. The
memory hog doesn't do any file IOs either. The IOs, and the resulting
dependence on storage performance, are due to the fact that memory
management and IO are intertwined. Your IO device is a critical component in
deciding how big your workload's memory footprint can be and how gracefully
the system can degrade under stress, with or without resource control.


___*The cgroup tree*___

It’s frustrating to watch a system back up like that: It seems obvious to us
that the management app is less important, more likely to cause issues, and
should have been throttled as the situation deteriorated.

But the system doesn't know that. To the kernel, they're all just processes:
Without resource control enabled, it has no mechanism to distinguish between
a primary workload, like rd-hashd in this case, and a malfunction like the
memory hog. They're equal in terms of memory and IO, and the kernel tries to
balance the two as best it can, as the system descends into a thrashing
live-lock.

cgroup (control group) is the Linux kernel feature that enables you to tell
the kernel how the system is composed, and how resources should be
distributed among the applications.

cgroup has a tree structure just like your filesystem, but each tree node -
called a cgroup - contains processes rather than files. This hierarchy tells
the kernel how processes are grouped into applications, and how applications
relate to each other. Along the hierarchy, cgroup controllers for CPU,
memory, and IO can be configured to monitor resource consumption, and
control resource distribution.

In systemd, intermediate cgroups are referred to as slices. In this demo,
we're primarily concerned with the top-level slices, or cgroups. Because
cgroup resource control is hierarchical, controlling how resources flow into
each top-level cgroup, and how the applications below them are organized,
allows us to control resource distribution across the system. The demo uses
the following top-level slices:

* workload.slice: This is where the system's primary workloads run. Our
  latency-sensitive primary workload - rd-hashd - runs here too as
  workload.slice/rd-hashd-A.service.

* sideload.slice: This is where secondary opportunistic side workloads run.
  We'll revisit sideloads later.

* hostcritical.slice: Some applications are critical for the basic health of
  the system. Here are some examples:

    * dbus: If dbus isn't responding, a systemd-based system might as well
      be off.

    * Fleet management software: Failures here can mean the machine isn't
      considered online at all.

    * System protection software like oomd: If these apps can't run, they
      can't protect the system.

    * Logging and logging in: If we can't log into the machine to read logs
      and debug, we can't fix problems. Some monitoring apps fall into this
      category too.

    * For the purposes of this demo, this program itself is a critical
      monitoring and control software. It also serves as a demonstration of
      what resource control can do: No matter what happens to the rest of
      the system, this demo will run and respond to you as long as resource
      control protections are enabled, as we'll do in the next section
      below.

* system.slice: This contains everything else: All the management,
  non-critical monitoring, plus the kitchen sink. We want these to run but
  it's not the end of the world if they slow down, or even get killed and
  restart if the situation gets really dire.

* init.scope: systemd lives here, which is critical for the whole system. We
  set up resource protection for this cgroup similar to hostcritical.slice.
  We won't get into details on this cgroup for now, but all the key points
  that apply to hostcritical.slice apply to this cgroup.

* user.slice: This is where the user's login sessions live. In some server
  deployments, this isn't used at all. In desktop setups, it's useful to
  configure it so that each session has some protection, and no single
  application suffocates the others. This demo sets up some basic
  protections for user.slice in case the demo is accessed from a local
  session, but detailed discussion of a desktop use case is out of scope for
  this demo.

The upper right pane in the demo shows various statistics for each top-level
cgroup. The last line tagged with "-" is the system total. Other graphs in
the graph view ('g'), plot the same metric for workload, sideload, and
system slices.

You can check out what cgroups are on your system by running `systemd-cgls`.
The kernel interface is a pseudo filesystem mounted under /sys/fs/cgroup.


___*Turning on resource protection*___

Once the kernel understands what applications are on the system and how
they're grouped in cgroups, it can monitor resource consumption and control
distribution along that hierarchy.

We'll repeat the same test run we did above, but leaving all the resource
protection configurations turned on.

rd-hashd should already be running at full tilt and all warmed up now, so
let's start the same memory hog, but without touch anything else:

%% on sysload memory-hog memory-growth-1x : [ Start memory hog ]

Monitor the RPS and other resource consumption in the graph view ('g'). The
RPS may go down a little and dip occasionally, but it'll stay close to full
capacity no matter how long you let the memory hog go on. Eventually the
memory hog will be killed off by oomd. Try it multiple times by clearing the
memory hog with the following button, and restarting it with the start
button above:

%% off sysload memory-hog       : [ Stop memory hog ]

In this scenario, with resource control on, our site's not going down, and
users probably aren't even noticing. If we have a working monitoring
mechanism, the oomd kills will raise an alarm, people can quickly find out
which program was malfunctioning, and hunt down the bug. What could have
been a site-wide outage got de-escalated into an internal oops.

This is cgroup resource protection at work. The kernel understood which part
was important and protected its resources at the expense of the memory hog
running in the low-priority portion of the system. Eventually, the situation
for the memory hog became unsustainable and oomd terminated it.

We'll go into details in the following pages but here's a brief summary of
how resource protection is configured in this demo.

* Memory:

  * workload.slice: memory.low is the best-effort memory protection⁠. It's
    set up so that most of the system's memory is available to the primary
    workloads if they want it.

  * hostcritical.slice: memory.min - the more strict version of memory.low -
    is set up so that 768MB is always available to hostcritical
    applications.

  * sideload.slice: memory.high is configured so that sideload doesn't
    expand to more than half of all memory. This isn't strictly necessary.
    Its only role is limiting the maximum size to which sideloads can expand
    when the system is otherwise idle, so that the primary workload doesn't
    have to wait for it to be kicked out while ramping up.

* IO: All configurations are through the io.cost controller and thus only
  have one value to configure⁠—the weight.

    workload : hostcritical : system : sideload = 500 : 100 : 50 : 1

  We want the majority to go to the workload. hostcritical shouldn't need a
  lot of IO bandwidth, but can still get a fair bit when needed. system can
  get one tenth of workload at maximum, which isn't huge, but is still
  enough to make non-glacial forward progress. sideload only gets what's
  left over.

  Note that the weight-based controllers such as io.cost are
  work-conserving. The ratio is enforced only when the underlying resource
  is contended. If the main workload is hitting the disk hard and a system
  service wants to use it at the same time, the system services would only
  be able to get up to 1/10 of what workload gets, but if the disk is not
  contended, system can use however much is available.

* CPU: All configurations are through cpu.weight.

    workload : hostcritical : system : sideload = 100 : 10 : 10 : 1

  The way cpu.weight works and is configured is very similar to IO.


___*Read on*___

For more details on cgroup:

 * Maximizing Resource Utilization with cgroup2\n
   https://facebookmicrosites.github.io/cgroup2/docs/overview

 * Control Group v2\n
   https://www.kernel.org/doc/Documentation/admin-guide/cgroup-v2.rst

Now that we have the basic understanding of cgroup and resource protection,
let's delve into details of how different parts of cgroup work.

%% jump comp.cgroup.mem          : * Memory Control
%% jump comp.cgroup.io           : * IO Control
%% jump comp.cgroup.cpu          : * CPU Control
%%
%% jump comp.cgroup.mem          : [ Next: Memory Control ]

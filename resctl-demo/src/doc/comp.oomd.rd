## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.oomd: OOMD - The Out-Of-Memory Daemon
%% reset prep
%% knob hashd-load 0.9
%% on hashd

*OOMD - The Out-Of-Memory Daemon*\n
*===============================*

Most of us have experienced a system getting bogged down by heavy memory
thrashing, then eventually becoming unresponsive, leaving hard reset the
only option. At other times, after some duration of stall, the kernel OOM
(Out-Of-Memory) killer kicks in, kills something, and saves the system.

Applications on a system can stake claims for more memory than they're going
to use and more than is available in the system. This overcommitment allows
the kernel to manage memory use automatically, without requiring inordinate
effort from each application to micro-manage every byte it uses. But if the
total hot working-set on the system significantly exceeds the available
memory, it can become difficult for the whole system to make meaningful
forward progress, leaving OOM kills as the only resolution.

Given the probabilistic nature of memory usage in many applications, OOM
kills can be considered an integral part of memory management. A fleet of
machines configured for a low rate of OOM kills can be doing more total work
than a one where everything is undercommitted.

But it's one thing to be short on memory and need to kill an application.
It's a completely different thing to grind the whole system to a halt,
affecting everything else on the system, and often requiring a hard reset.


___*The kernel OOM killer*___

The kernel OOM killer kicks in when it thinks that the system or the cgroup
isn't making forward progress. The kernel's definition of forward progress
is, understandably, narrow and conservative, since no one wants the kernel
to be killing processes willy-nilly. When the kernel literally can't run
because it can't allocate a page after trying pretty hard, it declares an
OOM condition.

Imagine a thread that's copying a long string from one location to another -
it repeatedly reads a byte from one location and then writes it to another.
Let's say the system is under such duress that the memory is taken away from
the thread at each step. When it reads a byte, it needs to fault that page
in from the filesystem. When it writes a byte, the page needs to be brought
back from swap. While the thread is waiting for one page, the other page
gets reclaimed, so each memory access needs to wait for the IO device.

The application is running magnitudes of order slower than normal and is
most likely completely useless. But to the kernel's eye, it is making
forward progress - one byte at a time - and thus it won't trigger an OOM
kill. Left alone, depending on the circumstances and luck, a condition like
this can last hours.


___*OOMD*___

OOMD is a userspace daemon that monitors various cgroup and system metrics,
including PSI, and takes remediating actions, including OOM killing. This
only works with system-wide resource control configured - the kernel makes
sure the system as a whole doesn't go belly up and OOMD itself can run
adequately. OOMD also provides additional protections, monitoring
application health, and taking action when app health deteriorates.

OOMD can be configured so it understands the topology of the system - which
parts are critical, and which can be killed and retried later - and applies
the matching application health criteria. For example, we can decide to
relieve contention by killing something if workload.slice is running at less
than a third of its maximum capacity for longer than a minute, even though
the condition is not even close to triggering the kernel OOM killer.

It can also take different actions depending on the situation, whether it's
notifying the appropriate task manager agent, or picking an OOM kill victim
based on the triggering condition.

resctl-demo has a very simple OOMD configuration:

* If workload.slice or system.slice thrashes for too long, kill the heaviest
  memory consumer in the respective slice.

* If swap is about to get depleted, kill the largest swap user.

The former resolves thrashing conditions early-on as they develop. The
latter protects against swap depletion, which warrants more discussion.


___*Swap depletion protection*___

There are two types of memory - file and anonymous. To over-simplify, the
former is a memory region created by mmap(2)'ing files, and the latter is
allocated through malloc(3). While there are differences in typical
read/write ratios, access locality, and background writeback mechanisms, the
actions necessary to reclaim the two types of memory are similar, by and
large. A page first needs to be made clean if dirty - i.e., it needs to be
stored into the backing store so its latest content can be brought back
later. Once clean, the page can be dropped and recycled.

When swap is not enabled or depleted, the anonymous part of memory
consumption becomes unreclaimable. It's essentially the same as mlock(2)'ing
all of anonymous memory. This greatly constrains the kernel's ability to
manage and arbitrate memory.

Imagine two cgroups with the same memory.low protection. cgroup A has mostly
file memory and cgroup B mostly anonymous. Both are expanding at a similar
rate. As long as swap is available, the kernel can be stay fair between the
two cgroups by applying similar levels of reclaim pressure. But when the
swap space runs out, suddenly all the memory B currently holds, along with
any future page it successfully allocates, will stay pinned to physical
memory. Only A's file pages are reclaimable, thus B will keep growing while
A keeps shrinking.

This effect can be drastic to the point where all memory protection in
system breaks down when swap runs out - making it possible for a low
priority cgroup with expanding anonymous memory to take down the whole
system. Note that this can happen without cgroups - unlimited mlocking is
bad news no matter what, but it becomes more apparent when using resource
control to push system utilization and reliability.

To prevent such situations, OOMD can be configured to monitor swap usage and
kill the biggest swap consumer when it gets too close to depletion. With an
adequate swap configuration, unless there's an obvious malfunction, it
should never run out, just as a functioning system should never run out of
filesystem space. When such malfunctions happen, killing by swap usage is an
effective way of detecting the culprit and resolving the issue.


___*OOMD in action - Swap depletion kill*___

This scenario might already seem familiar - rd-hashd running and a memory
hog going off in system.slice. We've used this to demonstrate memory and
other protection scenarios and didn't worry about system stability. Let's do
it again, but let's see what happens with rd-hashd's load reduced to 90% so
that filling-up swap doesn't take too long.

Once rd-hashd warms up and the memory usage stops expanding, let's start a
memory hog:

%% (                             : [ Start memory hog ]
%% reset secondaries
%% on sysload memory-hog memory-growth-1x
%% )

Watch the system.slice swap usage climbing. How quickly will depend on the
storage device performance. Eventually, it'll nearly fill up all the
available swap and you'll see something like the following in the
"Management logs" pane on the left:

  [15:33:24 rd-oomd] [../src/oomd/Log.cpp:114] 50.85 49.35 30.53 system.slice/rd-sysload-test-mem-1x.service 6905962496 ruleset:[protection against low swap] detectorgroup:[free swap goes below 10 percent] killer:kill_by_swap_usage v2

To view the full log, press 'l' and select "rd-oomd". This is OOMD killing
the memory hog due to swap depletion. The system weathered it just fine and
it didn't seem like much. Let's see what happens if we repeat the same
thing, but without OOMD:

%% (                             : [ Disable OOMD and start memory hog ]
%% reset secondaries
%% off oomd
%% on sysload memory-hog-1 memory-growth-1x
%% )

Observe how system.slice's memory usage keeps creeping up once swap is
depleted. Although workload.slice is protected, the memory hog's memory is
all mlocked and every page it gets, it gets to keep. This eventually
suffocates rd-hashd and pushes RPS down to 0. After that, depending on the
IO device performance and your luck, the kernel OOM killer might kick in and
resolve the situation, leaving something like the following in `dmesg`:

  [ 2808.411512] Out of memory: Killed process 45724 (memory-growth.p) total-vm:28805140kB, anon-rss:11815160kB, file-rss:4772kB, shmem-rss:0kB, UID:0 pgtables:55768kB oom_score_adj:0

Or the kernel might kill rd-hashd instead of the memory hog. There's also
some chance your system might completely lock up, requiring a hard power
cycle. If you want to try it again, reset the experiment with the following
button, wait for rd-hashd to recover, and then retry:

%% (                             : [ Prepare for swap depletion kill test ]
%% reset secondaries
%% reset protections
%% knob hashd-load 0.9
%% on hashd
%% )


___*OOMD in action - Pressure kill*___

Let's first reset experiments and ramp up hashd to 100%:

%% (                             : [ Prepare for pressure kill test ]
%% reset secondaries
%% reset protections
%% knob hashd-load 1.0
%% on hashd
%% )

Imagine a management application malfunctioning occasionally and bloating up
the memory it's actively using. If the system is generally tight on memory,
it'll soon start slowing down. With proper resource control in place, it
won't affect the main workload much, but the malfunctioning application
would become really slow.

While the main workload and the system as a whole are safe, this isn't
great. Whatever role the management application was performing, it isn't
now, and a crawling malfunction is often more difficult to detect.

This is what pressure-triggered kills are for. OOMD can monitor application
health through pressure metrics, and take action when they're clearly out of
an acceptable range. The following button starts a kernel compile job with
very high concurrency. The combination is a crude but effective stand-in for
the above scenario:

%% (                             : [ Start a compile job ]
%% on sysload compile-job build-linux-32x
%% )

It'll take a while to bloat, and system.slice's memory pressure will
gradually build up. Soon, it'll start running out of memory and experience
gradually increasing memory pressure. It eventually ends up waiting for IOs
most of the time, sustaining memory pressure close to 100%. As the OOMD
configuration is pretty conservative, it'll take some time, but OOMD will
kick in and terminate the offending process.

If the configuration and monitoring are set up correctly, the offending
application will be restarted as necessary, and alarms will be raised if
this happens at any scale.


___*Read on*___

We've examined each component of resource protection. On the next page,
we'll put them into action by experimenting with all the switches and
sliders.

%% jump prot.demo                : [ Next: Throwing Everything At It ]

## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.oomd: OOMD - The Out-Of-Memory Daemon
%% reset secondaries
%% reset protections
%% knob hashd-load 0.6
%% on hashd

*OOMD - The Out-Of-Memory Daemon*\n
*===============================*

Many of us experienced a system getting bogged down by heavy memory
thrashing and then eventually becoming unresponsive leaving hard reset the
only option. At other times, after some duration of stall, the kernel OOM
(Out-Of-Memory) killer kicks in and kills something and saves the system.

Applications on a system can stake claims for more memory than they're gonna
use and more than available in the system. This overcommitment allows the
kernel to manage memory usage automatically without requiring each
application to pay inordinate amount of effort micro-managing each byte it
uses. However, when the total hot working-set on the system significantly
significantly exceeds the available memory, it can become difficult for the
whole system to make meaningful forward progress leaving OOM kills as the
only resolution.

Given the probabilitic nature of memory usage in many applications, OOM
kills can be considered an integral part of memory management. Given a fleet
of machines, a configuration where there are a low rate of OOM kills can be
doing more total work than a configuration where everything is
undercommitted. However, being short on memory and having to kill an
application is one thing. Grinding the whole system down to a halt affecting
everything else on the system and often requiring hard reset is a completely
different thing.

So, why is that?


___*The kernel OOM killer*___

The kernel OOM killer kicks in when it thinks that the system or the cgroup
isn't making forward progress. The kernel's definition of forward-progress
is, understandably, narrow and conservative as no one wants the kernel to be
killing processes willy-nilly. When the kernel literally can't run because
it can't allocate a page after trying pretty hard, it declares an OOM
condition.

Imagine a thread which is copying a long string from one location to another
- it repeatedly reads a byte from one location and then writes it to
another. Let's say that the system is under such duress that the memory is
taken away from the thread at each step. When it reads a byte, it needs to
fault that page in from the filesystem. When it writes a byte, the page
needs to be brought back from swap. While the thread is waiting for one
page, the other page gets reclaimed, so each memory access needs to wait for
the IO device.

The application is running at a speed many magnitudes of order slower than
nominal and most likely completely useless. However, to the kernel's eye, it
is making forward progress - one byte at a time - and thus it won't trigger
an OOM kill. Left alone, depending on the circumstances and luck, a
condition like this can last hours.


___*OOMD*___

OOMD is a userspace daemon which monitors various cgroup and system metrics
including PSI and takes remediative actions including OOM killing. This only
works with system-wide resource control configured - the kernel makes sure
that the system as a whole doesn't go belly up and OOMD itself can run
adequately. On top of that, OOMD provides additional protection and monitors
application health and takes actions when they deteriorate too much.

OOMD can be configured so that it understands the topology of the system -
which parts are critical and which can be killed and retried later - and
apply the matching application health criteria. For example, we can decide
to relieve contention by killing something if workload.slice is running at
less than ja third of its maximum capacity for longer than a minute although
the condition is not even close to triggering the kernel OOM killer.

It also can take different actions depending on the situation whether that's
notifying the appropriate task manager agent or picking OOM kill victim
based on the triggering condition.

resctl-demo has a very simple OOMD configuration:

* If workload.slice or system.slice thrashes for too long, kill the heaviest
  memory consumer in the respective slice.

* If swap is about to get depleted, kill the largest swap user.

The former resolves thrashing conditions early-on as they develop and the
latter protects against swap depletio, which warrants more discussion.


___*Swap depletion protection*___

There are two types of memory - file and anonymous. To over-simplify, the
former is memory region created by mmap(2)'ing files while the latter is
allocated through malloc(3). While there are differences in typical
read/write ratio, access locality and background writeback mechanisms, by
and large, the actions necessary to reclaim the two types of memory are
similar. A page first needs to be made clean if dirty - ie. it needs to be
stored into the backing store so that its latest content can be brought back
later. Once clean, the page can be dropped and recycled.

When swap is not enabled or depleted, the anonymous part of memory
consumption becomes unreclaimable. It's essentially the same as mlock(2)'ing
all of anonymous memory. This greatly constrains the kernel's ability to
manage and arbitrate memory.

Imagine two cgroups with the same memory.low protection. cgroup A has mostly
file memory and cgroup B mostly anonymous. Both are expanding at a similar
rate. As long as swap is available, the kernel can be stay fair between the
two cgroups by applying a similar level of reclaim pressure. However, when
the swap space runs out, suddenly all the memory that B is currently holding
and any future page it'll succeed to allocate will stay pinned to physical
memory. Only A's file pages are reclaimable and thus B will keep growing
while A keeps shrinking.

This effect can be drastic to the point where all memory protection in
system breaks down when swap runs out - a low priority cgroup with expanding
anonymous memory can take down the whole system. Note that this can happen
without cgroups - unlimited mlocking is a bad news no matter what but the
issue becomes more apparent when trying to push system utilization and
reliability with resource control.

To prevent such situations, OOMD can be configured to monitor swap usage and
kill the biggest swap consumer when it gets too close to depletion. With the
adequate swap configurtion, unless there's an obvious malfunction, it should
never run out, just as a functioning system should never run out of
filesystem space. When such malfunction happens, killing by swap usage is an
effective way of detecting and resolving the culprit.


___*OOMD in action - Swap depletion kill*___

This scenario might already seem familiar - rd-hashd running and a memory
bomb going off in system.slice. We've used this to demonstrate memory and
other protection scenarios and didn't worry about system stability. Let's do
it again and see what happens but with rd-hashd's load reduced to 60% so
that filling-up swap doesn't take too long.

Once rd-hashd warms up, let's start a memory bomb.

%% (                             : [ Start memory bomb ]
%% reset secondaries
%% on sysload memory-bomb memory-growth-1x
%% )

Watch the system.slice swap usage climbing up. How quickly will depend on
the storage device performance. Eventually, it'll nearly fill up all the
available swap and you'll see something like the following in the
"Management logs" pane on the left.

  [15:33:24 rd-oomd] [../src/oomd/Log.cpp:114] 50.85 49.35 30.53 system.slice/rd-sysload-test-mem-1x.service 6905962496 ruleset:[protection against low swap] detectorgroup:[free swap goes below 10 percent] killer:kill_by_swap_usage v2

This is OOMD killing the memory bomb due to swap depletion. The system
weathered it just fine and it didn't seem like much. Let's see what happens
if we repeat the same thing but without OOMD.

%% (                             : [ Disable OOMD and start memory bomb ]
%% reset secondaries
%% off oomd
%% on sysload memory-bomb-1 memory-growth-1x
%% )

Observe how system.slice's memory usage keeps creeping up once swap is
depleted. Although workload.slice is protected, the memory bomb's memory is
all mlocked and every page it gets it gets to keep. This will eventually
suffocate rd-hashd and push down RPS to 0. After that, depending on the IO
device performance and your luck, the kernel OOM killer might kick in and
resolve the situation leaving something like the following in `dmesg`:

  [ 2808.411512] Out of memory: Killed process 45724 (memory-growth.p) total-vm:28805140kB, anon-rss:11815160kB, file-rss:4772kB, shmem-rss:0kB, UID:0 pgtables:55768kB oom_score_adj:0

Or there's some chance that your system might completely lock up requiring a
hard power cycle. If you want to try it again, reset the experiemnt with the
following button, wait for rd-hashd to recover and then retry.

%% (                             : [ Prepare for swap depletion kill test ]
%% reset secondaries
%% reset protections
%% knob hashd-load 0.6
%% )


___*OOMD in action - Pressure kill*___

Let's first reset experiments and ramp up hashd to 100%.

%% (                             : [ Prepare for pressure kill test ]
%% reset secondaries
%% reset protections
%% knob hashd-load 1.0
%% )

Imagine a management application malfunctioning occassionally and bloating
up the memory it's actively using. If the system is generally tight on
memory, it'll soon start slowing down. With proper resource control in
place, it won't affect the main workload much but the malfunctioning
application would become really slow.

While the main workload and system as a whole are safe, this isn't great.
Whatever role the management application was performing, it isn't now and a
crawling malfunction is often more difficult to detect.

This is what pressure-triggered kills are for. OOMD can monitor application
health through pressure metrics and take actions when they're clearly out of
acceptable range. The following button will start the same memory bomb but
make it access its memory pages frequently so that they can't be simply
swapped out. This is a crude but effective stand-in for the above scenario.

%% on sysload memory-bloat memory-bloat-1x : [ Start memory bloat ]

It will take a while to bloat up and system.slice's memory pressure will
gradually build up. Soon, it'll start running out of memory and experience
gradually increasing memory pressure. It eventually will be waiting for IOs
most of the time sustaining memory pressure close to 100%. As the OOMD
configuration is pretty convservative and acts on 1-min average pressure,
it'll take some time but OOMD will kick in and terminate the offending
process.

If the configuration and monitoring are set up correctly, the offending
application will be restarted as necessary and alarms will be raised if this
happens at any scale.


___*Read on*___

We've examined each component of resource protection. On the next page, put
them into action by experimenting with all the switches and sliders.

%% jump prot.demo                : [ Next: Throwing Everything At It ]
%% jump index                    : [ Exit: Index ]

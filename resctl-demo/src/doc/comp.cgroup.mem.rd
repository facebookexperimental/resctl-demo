## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.cgroup.mem: Memory Control
%% reset secondaries
%% reset protections
%% knob hashd-load 1.0
%% on hashd

*Memory Control*\n
*==============*

Most people have experienced memory shortages directly whether it's on
personal computers, servers or even phones. When the working set size
significantly exceeds the available memory, the system falls into a deep
thrashing state and moves at a glacial rate while the IO device is
constantly busy.

This is because once the working set of the workload doesn't fit available
memory, it has to be paged in from and out to the storage device on demand.
The aggressiveness of thrashing onset is determined by the workload's memory
access pattern and the performance gap between memory and the storage
device. For many workloads, the access pattern has mixture of hot and cold
areas. As the memory gets tighter, cold areas will get kicked out of memory
and faulted back in as needed.

As long as the storage device can keep up with the pace, the workload can
run fine. If memory squeezes further, the demand on the storage device will
keep rising. If the memory isn't enough to hold the hot areas, it can spike
abruptly. When that demand goes over what the storage device can do, the
workload gets slowed down drastically, its progress bound to page fault IOs.

The probablistic nature and cliff behavior can make memory sizing very
challenging. It's difficult to tell the amount of required memory and,
getting it wrong, even if just a little too low, can lead to drastic
failures. This is why this demo primarily uses the work-conserving
`memory.low` for memory control.

The Linux kernel has four memory control knobs - `memory.max`,
`memory.high`, `memory.min` and `memory.low`. The first two are limit
mechanisms while the latter two are protection.

`max` and `high` put hard limits on how much memory a cgroup and its
descendants can use. `max` triggers OOM-kills when the workload can't be
made to fit. `high` slows it down so that userspace agent can handle the
situation.

`min` and `low` work the other way around. Instead of limiting, they protect
memory for a cgroup and its descendants. If a cgroup has 4G `low` and is
currently using 6G, only the overage - 2G - is considered for memory
reclaim. It will experience the same level of reclaim pressure as an
unprotected peer cgroup using 2G. The difference between `min` and `low` is
that `low` can be ignored if the alternative is triggering OOM kills. For
brevity, we'll only discuss 'low' from now on.


___*memory.low*___

There are a couple great things about `low`.

First, it is work-conserving - if the protected cgroup doesn't use the
memory, others can use. Contrast this to memory limits - even if the system
is otherwise completely idle, the free memory can't be used if a cgroup has
reached its limit.

Second, it provides a gradient of protection. As usage grows past the
protected amount, the protected amount is still fully protected but the
cgroup will experience gradually increasing reclaim pressure matching the
amount of overage.

For example, a cgroup with 12G of protection using 15G is still gonna enjoy
a strong protection - it's only gonna experience the reclaim pressure
equivalent to a 3G cgroup, but if its requirement drops or the system goes
under extreme memory pressure, the cgroup will be able to yield some memory
to the most needed parts of the system.

Combined, this makes `low` easy and safe to use. No need to figure out the
exact amount. Instead, you can just say "prioritize 75% of memory for the
main workload and leave the rest for the usual usage-based distribution".
The main workload will then priorized and comfortably occupy most of the
system - it only has to compete in the top 25%, but if the situation gets
too tight for the rest of the system - the management portion, they'll be
able to wrangle out what's needed to ride out the tough times.

Compare the above to limits. The configuration would be the other way
around. The management portion would be limited to protect the main
workload. 25% limit for them doesn't make sense. Until the limit is reached,
reclaim pressure would applied based on comparative sizes - if main workload
is at 80% and the others at 20%, the main workload would be experiencing 4
times more reclaim pressure, which is clearly not what we want.

So, we need to set the management portion's limit lower, but how much lower?
Let's say 5% is usually enough and we set it to 5%. We're generally happy
but something new rolls out which temporarily needs a bit more than 5% and
the management portion goes belly up fleet-wide. We adjust it up a bit but
by how much? This cycle can eventually reach the point where the limit is
both too high for adequate workload protection and too low to avoid
noticeable increase in management operation failures.


___*Memory control configuration in this demo*___

This demo uses the following static memory control configuration.

 init.scope         : 16M  min  - systemd\n
 hostcritical.slice : 512M min  - dbus, journald, sshd, resctl-demo\n
 workload.slice     : 75%  low  - hashd\n
 sideload.slice     : No configuration\n
 system.slice       : No configuration\n

All that we're doing is setting up overall protections for the important
parts of the system in top-level slices. The numbers just need to be
reasonable ballparks. There isn't anything workload-dependent. All it's
saying is there are some critical parts of the system and majority of memory
should be used to run the main workloads. As such, the same configuration
can serve a wide variety of use cases as long as the way the system should
be used stays similar.

In the above configuration, hostcritical is rather large at 512M. This is
because the management agent and UI for this demo need to stay responsive at
all times and live under hostcritical.slice. In more usual setups,
hostcritical would be multiple times lower.


___*Memory protection in action*___

rd-hashd should already be running at full load. Once it warms up, let's
disable memory control and start memory bomb.

%% (                             : [ Disable memory control and start memory bomb ]
%% off io-resctl
%% on sysload memory-bomb memory-growth-1x
%% )

It goes south real fast. If something like this happens across many machines
in the fleet at the same time, it'll easily lead to site outage. Let's reset
the experiment and restore memory control.

%% (                             : [ Stop memory bomb and restore memory control ]
%% reset secondaries
%% reset protections
%% )

Wait for the sysload count to drop to zero and rd-hashd to stabilize and
then launch the same memory bomb again.

%% (                             : [ Start memory bomb ]
%% on sysload memory-bomb memory-growth-1x
%% )

RPS will drop a bit, which is expected - we want the management portion to
be able to use a small fraction of the system, but rd-hashd will stay close
to its full load while the malfunctioining system.slice is throttled so that
it can't overwhelm the system. Reset with the following button and repeat
the experiment.

%% reset secondaries             : [ Reset memory bomb ]

The system is hardly fazed by the memory bomb. Even if this happens on many
machines at the same time, the site is gonna be just fine. If we have
appropriate monitoring in place, we'd notice and investigate the problem and
follow up with fixes.


___*Read on*___

Ealier this page, we described memory shortage behaviors and briefly
explained how they happen. Because memory management is a critical and
challenging part in understanding and debugging resource related issues,
we'll delve into more details in the next page.

%% jump comp.cgroup.mem.thrash   : [ Next: The Anatomy of Thrashing ]
%% jump index                    : [ Exit: Index ]

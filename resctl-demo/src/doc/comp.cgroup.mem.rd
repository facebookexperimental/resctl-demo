## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.cgroup.mem: Memory Control
%% reset prep
%% knob hashd-load 1.0
%% on hashd

*Memory Control*\n
*==============*

Most people have experienced memory shortages - on personal computers,
Servers, or even phones. When a workload's working set size significantly
exceeds available memory, the system falls into a deep thrashing state and
moves at a glacial pace, while the IO device remains constantly busy. This
is because the memory has to be paged in from, and out to, the storage
device on demand when the working set exceeds available memory.

How aggressively the thrashing begins is determined by a combination of two
factors: The workload's memory access pattern, and the performance gap
between memory and the storage device. For many workloads, the access
pattern has a mixture of hot and cold areas. As the memory gets tighter,
cold areas get kicked out of memory and faulted back in as needed.

As long as the storage device keeps up with the pace, the workload can run
fine. If memory squeezes further, the demand on the storage device keeps
rising. If the memory isn't enough to hold the hot areas, demand can spike
abruptly. When demand exceeds the storage device's capabilities, the
workload slows to a crawl, its progress bound to page fault IOs.

The probabilistic nature and tendency to drastic behavior makes memory
sizing a challenge. Determining the optimal amount is difficult, and getting
it wrong - even just a little too low - can lead to catastrophic failures.

Cgroup2 recognizes this inherent difficulty and provides a robust and
forgiving way to allocate memory between cgroups: the work-conserving
`memory.low` control knob, which is the primary knob used for memory control
in this demo.

The Linux kernel has four memory control knobs - `memory.max`,
`memory.high`, `memory.min` and `memory.low`. The first two are limit
mechanisms, while the latter two are for protection.

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
brevity, we'll only discuss `low` from now on: See the Memory Controller
documentation for more detail:

https://facebookmicrosites.github.io/cgroup2/docs/memory-controller.html


___*memory.low*___

There are a couple great things about `low`.

First, it's work-conserving - if the protected cgroup doesn't use the
memory, it's available for others to use. Contrast this with memory limits,
where even if the system is otherwise completely idle, that free memory
can't be used if a cgroup has already reached its limit.

Second, it provides a gradient of protection. As a cgroup's usage grows past
the protected amount, the protected amount remains protected, but reclaim
pressure for the excess amount gradually increases.

For example, a cgroup with 12G of protection that's using 15G still enjoys
strong protection - it only experiences reclaim pressure equivalent to a 3G
cgroup. But if its requirement drops, or the system comes under extreme
memory pressure, the cgroup can still safely yield some memory to where it's
most needed by the system.

These two factors combined make `low` easy and safe to use. No need to
figure out the exact amount: You can just say "prioritize 75% of memory for
the main workload and leave the rest for the usual usage-based
distribution". The main workload will then prioritize and comfortably occupy
most of the system - it only has to compete in the top 25%. If the situation
gets too tight for the rest of the system, i.e., the management portion,
they'll be able to wrangle out what's needed to ride out the tough times.

Compare the above to limits, where the configuration is the other way
around: The management portion is limited to protect the main workload.
Because reclaim pressure is applied based on comparative size, a 25% limit
for them doesn't make sense: If the main workload is at 80% and the others
at 20%, the main workload experiences 4 times more reclaim pressure, which
is clearly not what we want.

So to compensate, we decide to set the management portion's limit lower, but
how much lower? Let's say 5% is usually enough and we set it to 5%. We're
generally happy but something new rolls out that temporarily needs a bit
more than 5% and the management portion goes belly up fleet-wide. We adjust
it back up a bit, but again, by how much? This cycle can eventually reach
the point where the limit is both too high for adequate workload protection
and too low to avoid noticeable increase in management operation failures.


___*Memory control configuration in this demo*___

This demo uses the following static memory control configuration:

 ___init.scope___             : 16M  min  - systemd\n
 ___hostcritical.slice___     : 768M min  - dbus, journald, sshd, resctl-demo\n
 ___workload.slice___         : 75%  low  - hashd\n
 ___sideload.slice___         : No configuration\n
 ___system.slice___           : No configuration\n

All we're doing is setting up overall protections for the important parts of
the system in top-level slices. The numbers just need to be reasonable
ballparks. There isn't anything workload-dependent. All it's saying is there
are some critical parts of the system, and most of the memory should be used
to run the main workloads. As such, the same configuration can serve a wide
variety of use cases, as long as the system's usage requirements are
similar.

In the above configuration, ___hostcritical___ is rather large at 512M. This
is because the management agent and UI for this demo live under
___hostcritical___ and need to stay responsive at all times. In more typical
setups, ___hostcritical___ is set multiple times lower.


___*Memory protection in action*___

Let's repeat a similar experiment as in the previous "Cgroup and Resource
Protection" section to demonstrate memory protection. rd-hashd should
already be running at full load. Once it warms up, disable memory control
and start a linux compile job with a ludicrous level of concurrency which
will viciously compete for memory:

%% (                         	: [ Disable memory control and start a compile job ]
%% off mem-resctl
%% on sysload compile-job build-linux-32x
%% )

Once the source tree is untarred and the compile commands start getting
spawned, ___system___'s memory pressure will shoot up. Soon after,
___workload___'s pressure will start climbing and rd-hashd's RPS slumping.

Let's stop the compile job and restore memory control:

%% (                         	: [ Stop the compile job and restore memory control ]
%% reset secondaries
%% reset protections
%% )

Wait for the sysload count to drop to zero and rd-hashd to stabilize. Once
rd-hashd's RPS is stable and memory footprint stops increasing, launch the
same compile job again:

%% (                         	: [ Start the compile job ]
%% on sysload compile-job build-linux-32x
%% )

RPS drops a bit, which is expected - we want the management portion to be
able to use a small fraction of the system, but rd-hashd will stay close to
its full load while the malfunctioning ___system___ is throttled so that it
can't overwhelm the system.


___*Read on*___

This page described memory shortage behaviors and briefly explained how they
happen, but because memory management is a critical and challenging part in
understanding and debugging resource issues, we'll delve into more details
on the next page.

%% jump comp.cgroup.mem.thrash   : [ Next: The Anatomy of Thrashing ]

## Copyright (c) Facebook, Inc. and its affiliates.
%% id side.intro: What is Sideloading?
%% reset prep
%% knob sys-cpu-ratio 0.01
%% knob sys-io-ratio 0.01
%% knob hashd-load 0.6
%% on hashd

*What is Sideloading?*\n
*====================*

___*DR-buffer*___

Given the high cost of buying and running machines, you want to use them to
their maximum potential all the time: Nobody wants to pay for capacity they
don't actually use. But the reality is that server fleet utilization often
falls far short of the ideal, with workloads commonly averaging below 50%
CPU utilization.

One reason for this is that sizing machine capacity for a set of workloads
is challenging, even when the workloads are fairly stable. Moreover, the
loads the workloads handle change often, in both predictable and
unpredictable ways - people go to sleep and wake up, local events can
trigger hot spots, a power or network outage can take out a data center,
shifting loads elsewhere - and so on.

The difficulty in sizing, combined with the inherent variability and
unpredictability, means average utilization can be pushed only so high, even
with careful bin-packing of workloads: There simply must be a significant
level of buffer to prevent service degradation, or even outages, caused by
inaccuracies in capacity sizing or unforeseeable spikes.

While disaster readiness isn't the only reason you need extra capacity, for
the sake of brevity, let's call it the disaster readiness buffer, or
DR-buffer. One requirement for a DR-buffer is that it must stay available
for unexpected surges. When a data center suddenly goes offline, the
remaining data centers must be able to pick up the extra load as quickly as
possible to avoid service disruption.

An important artifact of DR-buffer is that the idleness often leads to a
better latency profile, which is much desired for latency-sensitive
workloads.


___*Sideloading*___

Let's say your machines are loaded 60% on average, leaving 40% for the
DR-buffer and various other things: This isn't great, but not horrible
either. Note that while the CPUs are doing 60% of the total work they can
do, they might be reporting noticeably less than 60% CPU utilization: This
is a measurement artifact that we'll get back to later.

We have about 40% of compute capacity sitting idle and it would be great if
we could put something else on the system that could consume the idle
resources. The extra workload would have to be opportunistic, since it
doesn't have any persistent claim on resources - it would just use
whatever's left over in the system. Let's call the existing
latency-sensitive workload the main workload, and the extra opportunistic
one the sideload.

For sideloading to work, the following conditions should hold:

1. The DR-buffer should be available to the main workload on the system so
   it can ramp up unimpeded when needed.

2. Any impact the sideload might have on the latency benefit that the
   DR-buffer provides to the main workload should be limited and controlled.


___*A naive approach*___

In the previous chapter, we demonstrated that resource control can protect
the main workload from the rest of the system. While rd-hashd was running at
full load, we could throw all sorts of misbehaving workloads at the system
with only limited impact on the main workload. So what happens if we throw a
sideload at it? Can the same setup that worked against random misbehaving
workloads give the same protection against a sideload?

We already know that rd-hashd can be protected pretty well at full load.
Let's see how latency at 60% load and ramping up from there are impacted.

rd-hashd should already be running at 60% load. Once it's warmed up, start a
Linux build job with 2x CPU count concurrency. Pay attention to how the
latency in the left graph pane changes:

%% (                             : [ Start linux build job ]
%% on sysload compile-job build-linux-2x
%% )

Note how RPS is holding but latency deteriorates sharply. Press 'g' and
check out the resource pressure graphs. Even though this page set the CPU
weight of the build job only at a hundredth of rd-hashd, CPU pressure is
noticeable. We'll get back to this later.

Now let's push the load up to 100% and see whether its ability to ramp up is
impacted too:

%% knob hashd-load 1.0           : [ Set full load ]

It climbs, but seems kind of sluggish. Let's compare it with a load rising
but without the build job:

%% (                             : [ Stop linux build and set 60% load ]
%% off sysload compile-job
%% knob hashd-load 0.6
%% )

Wait until it stabilizes, then ramp it up to 100%:

%% knob hashd-load 1.0           : [ Set full load ]

Compare the slopes of RPS going up. The difference will depend on system
characteristics but there will be some.

So that didn't work very well. We want to utilize our machines efficiently,
but the noticeable increase in baseline latency is a high cost to pay, and
can be prohibitive for many use cases. The difference in ramp up is more
subtle, but still might be unacceptable.


___*Read on*___

Let's see how we can make this work in the next chapter.

%% jump side.sideloader          : [ Next: Sideloader ]

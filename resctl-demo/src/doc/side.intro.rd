## Copyright (c) Facebook, Inc. and its affiliates.
%% id side.intro: What is Sideloading?
%% reset secondaries
%% reset protections
%% knob sys-cpu-ratio 0.01
%% knob sys-io-ratio 0.01
%% knob hashd-load 0.6
%% on hashd
$$ reset resctl-params

*What is Sideloading?*\n
*====================*

___*DR-buffer*___

When you buy machines, you wanna use them to their maximum potentials all
the time. Nobody *wants* to pay for capacity that they don't actually use.
However, server fleet utilization often falls far short. CPU utilization
average staying below half is common for many workloads.

One cause is that sizing the machine capacity for a set of workloads can be
a challenging task even when the workloads are fairly stable. In addition,
for many use cases, the loads that the workloads have to handle often keep
changing in both predictable and unpredictable ways - people go to sleep and
wake up, local events triggering hot spots, a power or network outage taking
out a data center shifting loads elsewhere and so on.

The difficulty in sizing combined with inherent variability and
unpredictability mean that average utilization can be pushed up only so high
even with careful bin-packing of workloads. There just has to be a
significant level of buffer to weather inaccuracies in capacity sizing and
unforeseaable spikes to protect against service degradations including
outages.

Disaster readiness isn't the only reason why such extra capacity is needed
but, for the sake of brevity, let's call it disaster readiness buffer, or
DR-buffer. One requirement for DR-buffer is that it must stay available for
unexpected surges. When a data center suddenly goes offline, the remaining
data centers must be able to pick up the extra load as quickly as possible
to avoid service disruption.

One important artifact of DR-buffer is that the idleness often leads to
better latency profile which is much desired for latency-sensitive
workloads.


___*Sideloading*___

Let's say your machines are loaded 60% on average for DR-buffer and other
reasons, which isn't great but not horrible either. Note that while the CPUs
are doing 60% of total work it can do, it might be reporting a number
noticeably lower than 60% for CPU utilizaion. This is a measuring artifact
that we'll get back to later.

We have about 40% of compute capacity sitting idle and it'd be great if we
can put something else on the system which can consume the idle resources.
The extra workload would have to be opportunistic as it doesn't have any
persistent claim on resources - it's just using whatever is left over in the
system. Let's call the existing latency sensitive workload the main workload
and the extra opportunistic one sideload.

For sideloading to work, the followings should hold.

1. The DR-buffer should be available to the main workload on the system so
   that it can ramp up unimpeded when needed.

2. The impact on the latency improvements from DR-buffer should be limited
   and controlled.


___*A naive approach*___

In the previous chapter, we demonstrated that resource control can protect
the main workload from the rest of the system. While rd-hashd was running at
full load, we could throw all sorts of misbehaving workloads at the system
with limited impact on the main workload. If we replace misbehaving
workloads with a sideload, maybe that's just gonna work?

We already know that rd-hashd can be protected pretty well at full load.
Let's see how latency at and ramping up from 60% load level is impacted.

rd-hashd should already be running at 60% load. Once it's warmed up, let's
start a linux build job with 2x CPU count concurrency. Pay attention to how
the latency in the left graph pane changes.

%% (                             : [ Start linux build job ]
%% on sysload build-linux build-linux-2x
%% )

Look at how RPS is holding but latency deteriorates sharply. Press 'g' and
check out resource pressure graphs. Even though CPU weight of the build job
is only at a hundredth of rd-hashd, CPU pressure is noticeable. We'll get
back to this later.

Now let's push the load up to 100% and see whether its ability to ramp up is
impacted too.

%% knob hashd-load 1.0           : [ Set full load ]

It does climb but seems kinda sluggish. Let's compare it with load rising
without the build job.

%% (                             : [ Stop linux build and set 60% load ]
%% off sysload build-linux
%% knob hashd-load 0.6
%% )

Wait until it stabilizes and ramp it upto 100%.

%% knob hashd-load 1.0           : [ Set full load ]

Compare the slopes of RPS going up. The difference will depend on system
characteristics but there will be some.

That didn't work that well. We want to utilize our machines efficiently but
the noticeable increase in baseline latency is a high cost to pay and can be
prohibitive for many use cases. The difference in ramp up is more subtle but
still may not be acceptable.


___*Read on*___

Let's see how we can make this work in the next chapter.

%% jump side.sideloader          : [ Next: Sideloader ]
%% jump prot.demo                : [ Prev: Throwing Everything At It ]
%% jump index                    : [ Exit: Index ]

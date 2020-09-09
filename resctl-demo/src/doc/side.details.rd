## Copyright (c) Facebook, Inc. and its affiliates.
%% id side.details: Some Details on Sideloading
%% reset secondaries
%% reset protections
%% knob sys-cpu-ratio 0.01
%% knob sys-io-ratio 0.01
%% knob hashd-load 0.6
%% on hashd
$$ reset resctl-params
$$ reset graph

*Some Details on Sideloading*\n
*===========================*

Let's delve into some details that we skimmed over previously.

___*CPU utilization and actual amount of work done*___

hashd RPS can be a metric for the actual computation done. Each request
calculates sha1 of data blocks, and the numbers of bytes follow a normal
distribution. While there's some influence from increasing the memory
footprint as RPS grows, the differences are minor, usually a low single
digit percentage. If a machine is specified to perform 100 RPS when all CPUs
are fully saturated, at 50 RPS it would be doing around half the total
computation it can.

Previously, we noted that the usual CPU utilization percentage, which is
measured in wallclock time, doesn't scale linearly with the total amount of
work the CPUs can do. We can observe this relationship by varying the load
level of hashd.

hashd is already running at 60% load. Let's switch to the RPS / CPU util
graph for direct comparison:

%% graph RpsCpu                  : [ Switch to RPS / CPU graph ]

Look at the CPU utilization: It's likely significantly lower, though it will
vary by CPU. Now, increase the load level gradually with the knob below and
see how it changes:

%% knob hashd-load               : hashd load % :

On most hardware, CPU utilization will stay significantly lower than the
load level, until the load level crosses 80% or 90%, then it quickly catches
up. How they exactly map will depend on the specific hardware and workload.

Let's reset and continue on to the next section:

%% (                             : [ Reset graph and load level ]
%% knob hashd-load 0.6
%% reset graph
%% )


___*CPU sub-resource contention*___

Let's see whether we can demonstrate the effect of CPU sub-resource
contention.

The RPS determines how much computation rd-hashd is doing. While memory and
IO activities have some effect on CPU usage, the effect isn't significant
unless the system is under heavy memory pressure. So, we can use RPS as the
measure for the total amount of work the CPUs are doing.

rd-hashd should already be running at 60% load. Once it warms up, note the
CPU utilization level of workload.slice: It should be fairly stable. Now,
let's start the Linux build job as sysload - no CPU headroom - and see how
that changes:

%% (                             : [ Start linux build sysload ]
%% reset secondaries
%% on sysload build-linux build-linux-2x
%% )

Wait until the compile phase starts and system.slice's CPU utilization rises
and stabilizes. Compare workload.slice's current CPU utilization to before:
The RPS didn't change but its CPU utilization rose. The CPUs are taking
significantly more time doing the same amount of work. This is one of the
major contributing factors for the increased latency.

Let's start it as a sideload - with CPU headroom - and see whether there's
any difference:

%% (                             : [ Stop linux build sysload and start it as sideload ]
%% reset secondaries
%% on sideload build-linux build-linux-2x
%% )

Once the compile phase starts, workload.slice's CPU utilization rises, but
noticeably less compared to the prior attempt without CPU headroom. You can
tune the headroom amount with the following slide. Nudge it up and down, and
observe how workload.slice's CPU utilization responds:

%% knob cpu-headroom             : CPU headroom :

The specifics will vary by CPU, but the relationship between headroom and
main workload latency usually resembles a hockey stick curve. As headroom is
reduced, there's a point where latency impact starts increasing noticeably.
This is also the point where the CPUs are actually starting to get
saturated, and where increasing the amount of work contributes more to
overall slower execution rather than increased bandwidth.


___*How much actual work is the sideload doing?*___

Pushing up utilization with sideloading is nice, but how much actual work is
it getting out of the system? Let's compare the completion times of a
shorter build job, when it can take up the whole system vs. running as a
sideload:

%% (                             : [ Stop hashd and start allnoconfig linux build sysload ]
%% off hashd
%% reset secondaries
%% on sysload build-linux-min build-linux-allnoconfig-2x
%% )

Monitor the progress in the "other logs" pane on the left. Depending on the
machine, the build will take some tens of seconds. When the job finishes, it
prints out how long the compilation part took, in a line similar to
"Compilation took 10 seconds". If it's difficult to find in the left pane,
open log view with 'l' and select rd-sysload-build-linux-min, and record the
duration. This is our baseline - the time it takes to build allnoconfig
kernel, when it can take up the whole machine.

Now, let's try running it as a sideload. First, start hashd at 60% load:

%% (                             : [ Start hashd at 60% load ]
%% knob hashd-load 0.6
%% on hashd
%% )

Let it ramp up to the target load level. As our only interest is CPU, we
don't need to wait for the memory footprint to grow. Now, let's start the
build job again:

%% (                             : [ Start allnoconfig linux build sideload ]
%% reset secondaries
%% on sideload build-linux-min build-linux-allnoconfig-2x
%% )

Wait for it to finish and note the time as before. The log for this run is
in rd-sideload-build-linux-min.

On a test machine with AMD Ryzen 7 3800X (8 cores and 16 threads), the full
machine run took 10s, while the sideloaded one took 30s. The number is
skewed against the full machine run because the build job is so short and
there are phases that aren't parallel, but we could get around 1/3 of full
machine capacity while running it as a sideload, which seems roughly in the
ballpark given that the main workload was running at 60% of full machine
capacity, but kinda high given that we were running with 20% headroom.

Let's try the same thing with a longer build job. If you're short on time,
feel free to skip the following experiment and just read the results from my
test machine:

%% (                             : [ Stop hashd and start defconfig linux build sysload ]
%% off hashd
%% reset secondaries
%% on sysload build-linux-def build-linux-defconfig-2x
%% )

Wait for completion and take note of how long compilation took and then
start hashd at 60% load:

%% (                             : [ Start hashd at 60% load ]
%% knob hashd-load 0.6
%% on hashd
%% )

Once it warms up, start the same build job as a sideload:

%% (                             : [ Start defconfig linux build sideload ]
%% reset secondaries
%% on sideload build-linux-def build-linux-defconfig-2x
%% )

On a test machine, the full machine run took 81 seconds; the sideload run
305 seconds. That's ~27%. 60% for hashd + 27% for the sideload adds up to
87% - still higher than expected given the 20% headroom. While experiment
errors could contribute some, the total amount of work done being higher
than raw utilization number is expected, given that the machine reaches
saturation before wallclock-measured utilization hits 100%.

This result indicates that we can obtain almost full utilization of the
machine without sacrificing much. The only cost we had to pay was less than
5% increase in latency, and we got more than 25% extra work out of the
machine which was already 60% loaded - a significant bang for the buck. If
the average utilization in your fleet is lower, which is often the case, the
bang is even bigger.


___*Read on*___

We examined the CPU utilization number and the actual amount of work done,
CPU sub-resource contention, and how much extra work can be extracted with
sideloads. If you're itching to test your own sideloading scenarios, proceed
to the next page.

%% jump side.exp                 : [ Next: Experiment with Sideloading ]
%% jump index                    : [ Exit: Index ]

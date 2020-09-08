## Copyright (c) Facebook, Inc. and its affiliates.
%% id intro.iocost: Iocost Parameters and Benchmark
%% graph IoUtil
$$ reset graph

*The cgroup2 IO cost model based controller*\n
*==========================================*

___*Overview*___

The iocost controller uses an IO cost model to estimate the cost of each IO,
and implements work-conserving proportional control based on the estimated
cost. Each IO is classified as sequential or random and given a base cost
accordingly. On top of that, a size cost proportional to the length of the
IO is added. While simple, this model sufficiently captures the operational
characteristics of a wide variety of devices.

For more high-level explanations of IO control and the io.cost controller,
see the following page.

%% jump comp.cgroup.io           : [ IO Control ]


___*The parameters*___

While the kernel comes with a few sets of default parameters, to achieve a
reasonable level of control, the IO cost model should be configured in
`/sys/fs/cgroup/io.cost.model` according to the specific device, with the
following parameters:

* rbps      - Maximum sequential read BPS\n
* rseqiops  - Maximum 4k sequential read IOPS\n
* rrandiops - Maximum 4k random read IOPS\n
* wbps      - Maximum sequential write BPS\n
* wseqiops  - Maximum 4k sequential write IOPS\n
* wrandiops - Maximum 4k random write IOPS

The cost model is of course an approximation of reality and can't exactly
predict how the hardware is going to behave, especially as the devices
themselves show dynamic performance deviations over time. The controller
adapts to the situation by scaling the total command issue rate according to
the Quality-of-Service parameters - the latency targets and vrate bounds.
The following parameters are configured in `/sys/fs/cgroup/io.cost.qos`.

* rpct      - Read latency percentile to use\n
* rlat      - Read target latency\n
* wpct      - Writ elatency percentile to use\n
* wlat      - Write target latency\n
* min       - vrate bound minimum\n
* max       - vrate bound maximum

The latency targets determine when the controller considers the device fully
saturated. For example, "rpct=95 rlat=5000" means that if the 95th
percentile of read completion latency is above 5ms, the device is at
capacity and command issuing should be throttled.

The QoS vrate bounds express the percentage range of how much the device may
be throttled up and down to meet the latency targets. For example, a range
of 50% - 125% tells the controller to adjust maximum command issue rate
between half and 1.25x of what would add up to 100% according to the cost
model parameters. If `rbps` is 400MBps and the workload is only doing
sequential read, depending on the completion latency, the iocost controller
will allow issuing between 200MBps and 500MBps.

The QoS parameters are affected by both the device itself and, to a lesser
extent, the requirements of the workloads. In most cases, a device's latency
response graph has a point where latency takes off. The device is already
saturated, and adding more concurrent commands simply increases the latency.
Setting target latencies around that point is one way to configure the QoS
parameters.

Another interesting aspect is that vrate range can play a guiding role for
the underlying device. For example, some SSDs can complete a lot of writes
at a very high speed for a short time and then go into a semi-comatose
state, failing to complete other commands for hundreds or even thousands of
milliseconds. While such bursts might look good on simple short benchmarks,
they don't bring a lot of practical benefits and are deterimental to any
latency-sensitive workloads which may end getting hit by the following
stalls. iocost can avoid such irregularities by limiting vrate max close to
100% so that no matter how quickly the device signals write completion, the
system never issues more than it can sustain.

There are also SSDs that show significantly raised latencies for a while no
matter how few IOs are thrown at it, likely during a certain phase of
garbage collection. In such cases, scaling down command issue rate further
and further doesn't gain anything while losing the total amount of work. The
vrate min bound can protect against such temporary extreme cases.

These are a lot of numbers to configure but they're for the most part device
model specific. In the future, we're hoping to build a database with known
devices and their parameters so they can be configured automatically.


___*The benchmark*___

`/var/lib/resctl-demo/misc-bin/iocost_coef_gen.py` runs as
`rd-bench-iocost.service` and determines both the cost model and QoS
parameters.

The QoS parameters are calculated as 4 times the random IO completion
latency at 90% load and the vrate range is between 25% and 90%. The formulas
are derived empirically to achieve reliable demo behavior across a wide
variety of devices and may not be optimal for other use cases.

Once the benchmarks are complete, the results will be recorded in
`/var/lib/resctl-demo/bench.json` and propagated to
`/sys/fs/cgroup/io.cost.model` and `/sys/fs/cgroup/io.cost.qos`. If you edit
the file, the kernel configurations will be updated accordingly.

You can re-run and cancel hashd benchmark with the following.

%% toggle bench-hashd            : Toggle iocost benchmark


___*Read on*___

For more high-level explanations of IO control and the io.cost controller,
see the following pages.

%% jump comp.cgroup.io           : [ IO Control ]
%%
%% jump intro.hashd              : [ Next: rd-hashd Workload Simulator ]
%% jump intro.pre-bench          : [ Back: Benchmarks ]
%% jump index                    : [ Exit: Index ]

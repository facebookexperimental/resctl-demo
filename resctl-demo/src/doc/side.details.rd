## Copyright (c) Facebook, Inc. and its affiliates.
%% id side.details: Some Details on Sideloading
%% reset secondaries
%% reset protections
%% knob sys-cpu-ratio 0.01
%% knob sys-io-ratio 0.01
%% knob hashd-load 0.5
%% on hashd
$$ reset resctl-params

*Some Details on Sideloading*\n
*===========================*

Let's delve into some details that we skimmed over in the previous page.

___*CPU sub-resource contention*___

Let's see whether we can demonstrate the effect of CPU sub-resource
contention.

The RPS determines how much computation rd-hashd is doing. While memory and
IO activities have some effect on CPU usage, the effect isn't significant
unless the system is under heavy memory pressure. So, we can use RPS as the
measure for the total amount work the CPUs are doing.

rd-hashd should already be running at 50% load. Once it warms up, note the
level of workload.slice CPU utilization. It should be fairly stable. Now,
let's start linux build job as sideload and see how that changes.

%% (                             : [ Start linux build job ]
%% on sideload build-linux build-linux-2x
%% )

Wait until the compile phase starts and system.slice's CPU utilization rises
and stabilises. Compare the current CPU utilization of workload.slice to
before. The RPS didn't change but its CPU utilization rose - the CPUs are
taking significantly more time doing the same amount of work. This is the
main contributing factor for the increased latency.




___*Read on*___

%% jump side.sideloader          : [ Prev: Sideloader ]
%% jump index                    : [ Exit: Index ]

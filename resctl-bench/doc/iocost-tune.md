`iocost-tune` benchmark
=======================

`iocost-tune` analyzes the results of an `iocost-qos` benchmark to identify
behavior characteristics of the IO device and compute iocost QoS parameter
solutions. If the specified bench series doesn't include a preceding
`iocost-qos` instance, `iocost-tune` runs `iocost-qos` as follows:

```
   iocost-qos:dither,vrate-max=125.0,vrate-intvs=25
```

Analyzed Metrics
================

By default, `iocost-tune` analyzes how the following metrics change as vrate
is throttled:

#### MOF (Memory Offloading Factor)

How much of `rd-hashd` memory footprint can be offloaded to the IO device.
This is a latency-limited bandwidth performance metric. See the `common` doc
and `resctl-demo` for more information on memory offloading.

#### aMOF (Adjusted Memory Offloading Factor)

How much of `rd-hashd` memory footprint can be offloaded to the IO device
while being able to protect `rd-hashd` against interferences. This is always
equal to or less than `MOF` for the same vrate. For latency critical use
cases, this is the memory footprint that can be supported safely by the IO
device.

#### aMOF-delta (Adjusted Memory Offloading Factor Delta)

The difference between `MOF` and `aMOF-delta`. The wider the delta, the more
difficult it is to size the workload for protection as a size which
saturates the machine will be too big to protect.

#### isol-01 (Isolation Factor)

Isolation factor is defined as

```
   MEASURED_RPS / TARGET_RPS
```

and indicates the quality of protection. It's measured every second and one
of the percentiles (the 1st by default) is compared against the threshold
(90% by default) to determine whether protection is good enough.

This is what guides whether `aMOF` needs to be pushed lower. If the recorded
value for a given vrate is lower than the threshold, it indicates that
sufficient protection couldn't be achieved even at the smallest workload
size.

#### `lat-imp` (Latency Impact)

Latency impact is defined as

```
   (MEASURED_LATENCY - BASELINE_LATENCY) / BASELINE_LATENCY
```

where latency is the end-to-end `rd-hashd` request completion latency.

#### `work-csv` (Work Conservation)

Measures how much IO bandwidth the kernel was able to preserve while
protecting against memory hog. The lossage is caused by inefficiency in the
current implementation of anonymous memory throttling and doesn't reflect IO
device characteristics.

#### `rlat-XX-YY` and `wlat-XX-YY` (Read and Write Latencies)

IO read and write completion latencies. See `common` doc for more info.


Solutions
=========

The following iocost QoS solutions are computed by default. Note that the
descriptions of the solution logics aren't comprehensive.

#### `naive`

It targets 100% of what the model parameters describe (`fio` measured
maximum). vrate will be throttled down to 75% based on the p99 read and
write latencies.

#### `bandwidth`

This targets the maximum vrate at which `rd-hashd` can be isolated
sufficiently - isol-01 >= 90%. Sizing memory footprint may be challenging
with this solution - a workload sized for saturation may be too big for
isolation.

#### `isolated-bandwidth` 

This targets the maximum vrate which provides the lowest latency impact,
clamped between the `isolation` and `bandwidth` solutions. This is the vrate
below which the isolation quality doesn't improve.

#### `isolation`

This targets the maximum vrate which renders the minimum aMOF-delta. Sizing
for isolation is the easiest with this solution - a workload sized for
saturation is as close to be isolable as possible on the device.

#### `rlat-99-q[1-4]`

Each of these solutions targets a quarter of the 99th percentile read
latency spread. `q1` targets 100% vrate and modulates it down to 75% point,
and then `q2` starts there and so on. These parameters can be useful for
trying out and seeing what would work if the other solutions aren't
available or adequate.


Reading Results
===============

When `format` subcommand is used to print the full result, graphs like the
followings are printed:

```
   $ resctl-bench -r result.json format iocst-tune
```

```
       |
       |
       |
       |
       |
   1.6-|                                                                       ●
       |                                       ●             ●
       |                                  ■■●■■■■■■■■■■●■■■■■■■■■■■●■■■■■■■■■■■
       |                                 ■        ● ●     ●     ●           ●
 M     |                                                                 ●
 O     |                                 ●                            ●
 F 1.4-|                              ● ■
 @     |            ●                  ■
 1     |                              ■
 6     |                             ■
       |
       |                            ■
       |          ●                ■
   1.2-|       ■■■■■■■■●■■■■■■■■■■■●
       |                  ●  ●  ●
       |
       |       ●
       |
       |
     1+--------------------------------------------------------------------------------
      |                      |                       |                      |
      0                     40                      80                     120
          vrate 14.7-124.7 (min=1.190 max=1.509 L-infl=48.4 R-infl=62.1 err=0.012)
```

The circles are the data points from `iocost-qos` results and the squares
form the fitted line, which is used to interpret the noisy source data. The
above is showing how MOF changes at different vrates. We can see that the
right inflection point is at the vrate 62.1%, which according to the above
description should be the `bandwidth` solution.

In the `Solutions` section, we can find the matching solution:

```
   [bandwidth] MOF=max
     info: scale=68.35% MOF=1.509@16 aMOF=1.287 aMOF-delta=0.118 isol-01=91.83%
     rlat: 50-mean= 221u 50-99= 469u 50-100= 947u 99-mean= 3.6m 99-99=12.3m 100-100= 294m
     wlat: 50-mean=34.5u 50-99= 189u 50-100= 781u 99-mean= 597u 99-99= 8.3m 100-100= 363m
     model: rbps=1454473514 rseqiops=156751 rrandiops=152357 wbps=678545224 wseqiops=145498 wrandiops=62214
     qos: rpct=0.00 rlat=3647 wpct=0.00 wlat=597 min=100.00 max=100.00
```

`scale=68.35` is showing how much the solution is throttling from the
original model parameters and should match the vrate from the MOF right
inflection point. However, the inflection point was 62.1% and our solution
is 68.35%. This is because the solution is applying some heuristics to avoid
sitting right on top of the steep slope based on the steepness of the slope
and variance.

The `model` and `qos` lines are the determined parameters that can be fed to
the kernel. For example, to apply to `nvme0n1` which has the device number
`259:0` and enable:

```
   $ echo '259:0 rbps=1454473514 rseqiops=156751 rrandiops=152357 wbps=678545224 wseqiops=145498 wrandiops=62214' > /sys/fs/cgroup/io.cost.model
   $ echo '259:0 enable=1 rpct=0.00 rlat=3647 wpct=0.00 wlat=597 min=100.00 max=100.00' > /sys/fs/cgroup/io.cost.qos
```

Note that the QoS `min` and `max` are fixed at 100% instead of 68.35%. This
is because the model parameters are scaled instead. `iocost-tune` always
scales the model parameters so that the QoS `max` always ends up 100%.

`iocost-tune` can also generate a pdf file containing all the results:

```
   $ resctl-bench -r result.json format iocost-tune:pdf
```


Merging
=======

`iocost-qos` benchmark result can be noisy form SSD behavior inconsistencies
and other system behavior variances. While `iocost-tune` tries its best to
make sense of the noisy data, nothing improves solution quality like more
data points.

While increasing the number of `iocost-qos` intervals is one way to obtain
more data points, the default 25 interval runs can already take more than
six hours. `iocost-tune` supports result merging so that data points from
multiple separate benchmark runs can be combined to yield more accurate
results.

For example, the following command merges the results in `result-0.json`,
`result-1.json` and `result-2.json` into `merged.json`.

```
   $ resctl-bench -r merged.json merge result-0.json result-1.json result-2.json
```

Note that the command isn't specifying the benchmark type to merge.
`resctl-bench` automatically merges all results which are mergeable, groups
them into source groups and merges them. The `iocost-tune` source results
are grouped by:

* Memory profile.
* Storage device model.
* Benchmark ID if `--by-id` is specified.
* `resctl-bench` version unless `--ignore-versions` is specified.
* `iocost-qos` bench properties except for `vrate-intvs`.

If `--multiple` is specified, all source groups are merged; otherwise, one
group with the most number of sources is selected and merged.

Merging records and reports what happened in `merge-info`, a pseudo
benchmark, result.

```
   [merge-info result] 2021-06-18 14:29:41 - 14:29:41

   [0] iocost-tune
     version: 1.0.0 x86_64-unknown-linux-gnu
     memory-profile: 16
     storage: WDC CL SN720 SDAQNTW-512G-1020
     classifier: dither,vrate-max=125
     sources:
       + result-0.json
       + result-1.json
       + result-2.json
```


Properties
==========

First group properties (applies to all sub-runs)
------------------------------------------------

#### `gran` (float, default: 0.1)

The granularity used when fitting lines to data points. The finer the
granularity, the more cycles are needed.

#### `scale-min` (fraction, default: 0.01)

The minimum scale factor. No solution will scale below. See `scale-max`.

#### `scale-max` (fraction, default: 1.0)

The maximum scale factor. No solution will scale above. 1.0 means that the
solution won't ever scale up the model parameters.

#### Additional data set selector

Specify additional data sets to analyze:

* `isol-mean`: Average isolation factor
* `isol-PCT`: PCT'th percentile isolation factor
* `rlat-LAT_PCT-TIME_PCT`: IO read completion latencies. See `common` for
  details.
* `wlat-LAT_PCT-TIME_PCT`: IO write completion latencies. See `common` for
  details.


Second+ group properties
------------------------

Each group represents one QoS solution to compute. Every group should have
one `name` property and zero or one of the QoS solution target properties.
If no QoS solution target is specified, the `naive` solution is computed.

#### `name` (string)

The name of the solution.

#### `vrate` (vrate range), `rpct` (latency percentile), `wpct` (latency_percentile)

Manual vrate range with `rpct` and/or `wpct` based dynamic adjustment. For
example:

```
   $ resctl-bench -r merged.json solve 'iocost-tune::name=test,vrate=75-100,rpct=50,wpct=0'
```

produces a solution which is adjusted according to 50th percentile read
latency between 75% and 100%:

```
   [test] vrate=75-100, rpct=50
     info: scale=100.0% MOF=1.479@16 aMOF=1.269 aMOF-delta=0.221 isol-01=92.51%
     rlat: 50-mean= 225u 50-99= 713u 50-100= 1.9m 99-mean= 3.8m 99-99=13.1m 100-100= 346m
     wlat: 50-mean=54.9u 50-99= 305u 50-100=13.0m 99-mean= 1.4m 99-99=22.7m 100-100= 378m
     model: rbps=2127854279 rseqiops=229322 rrandiops=222894 wbps=992692782 wseqiops=212859 wrandiops=91017
     qos: rpct=50.00 rlat=225 wpct=0.00 wlat=0 min=75.00 max=100.00
```

#### `rlat-LAT_PCT` and `wlat-LAT_PCT` (fraction range or q[1-4])

vrate range which maps to the specified segment of the latency slope. For
example:

```
   $ resctl-bench -r merged.json solve 'iocost-tune::name=test,rlat-99=q2'
```

is equivalent to

```
   $ resctl-bench -r merged.json solve 'iocost-tune::name=test,rlat-99=50%-75%'

```

and produces

```
   [test] rlat-99=0.5-0.75
     info: scale=55.92% MOF=1.402@16 aMOF=1.269 aMOF-delta=0.087 isol-01=94.88%
     rlat: 50-mean= 198u 50-99= 431u 50-100= 744u 99-mean= 3.3m 99-99=13.1m 100-100= 253m
     wlat: 50-mean=54.9u 50-99= 305u 50-100= 2.6m 99-mean= 742u 99-99= 9.8m 100-100= 378m
     model: rbps=1189789720 rseqiops=128225 rrandiops=124631 wbps=555064169 wseqiops=119020 wrandiops=50892
     qos: rpct=99.00 rlat=3265 wpct=0.00 wlat=0 min=75.45 max=100.00
```

#### `mof=max` and `amof=max`

The minimum vrate point where the specified MOF is at maximum.

#### `isolated-bandwidth` and `isolation`

Solves for the `isolated bandwidth` and `isolation` solution described above
respectively.


Format properties
-----------------

#### `pdf` (String)

Generate a pdf file containing the result summary and graphs. If no value is
specified, `RESULT_PATH_STEM.pdf` is used where `RESULT_PATH_STEM` is the
file stem of the global `--result` path.

# `iocost-qos` benchmark

This benchmark utilizes `storage` and `protection:mem-hog-tune` to evaluate
different iocost QoS configurations. For each QoS configuration,
`iocost-qos` determines the following two parameters:

* MOF (Memory Offloading Factor): This is measured by running `storage` and
  indicates how much of `rd-hashd`'s memory footprint the IO device can
  offload without missing latency targets.

* aMOF (Adjusted Memory Offloading Factor): This is measured by using the
  `protection:mem-hog-tune` benchmark to find the largest memory footprint
  that can be protected sufficiently. The probing starts at the size
  determined by the `storage` benchmark and ends slightly below the
  available memory. This value indicates how much of `rd-hashd`'s memory
  footprint the IO device can offload without missing latency targets while
  protecting it against interferences.

This benchmark takes multiple property groups and the second group and on
specify the QoS configurations to probe. For example,

```
$ resctl-bench -r result.json run \
  iocost-qos::iocost-qos::min=100,max=100:min=80,max=80:min=60,max=60:min=40,max=40:min=20,max=20
```

probes the baseline (iocost off) and then vrate at 100%, 80%, 60%, 40% and
20%. This is equivalent to

```
$ resctl-bench -r result.json run \
  iocost-qos:vrate-min=20,vrate-max=100,vrate-intvs=5
```

which in turn is the default and thus the same as

```
$ resctl-bench -r result.json run iocost-qos
```

When the benchmark starts, it prints out what it's going to probe as
follows:

```
[INFO] iocost-qos[00]: -  iocost=off
[INFO] iocost-qos[01]: -  rpct=50.00 rlat=217 wpct=50.00 wlat=161 [min=100.00] [max=100.00]
[INFO] iocost-qos[02]: +  rpct=50.00 rlat=217 wpct=50.00 wlat=161 [min=80.00] [max=80.00]
[INFO] iocost-qos[03]: +  rpct=50.00 rlat=217 wpct=50.00 wlat=161 [min=60.00] [max=60.00]
[INFO] iocost-qos[04]: +  rpct=50.00 rlat=217 wpct=50.00 wlat=161 [min=40.00] [max=40.00]
[INFO] iocost-qos[05]: -s rpct=50.00 rlat=217 wpct=50.00 wlat=161 [min=20.00] [max=20.00]
[INFO] iocost-qos: 3 storage and protection bench sets to run, isol-01 >= 90.00%
```

Each of the first six lines starting with `iocost-qos[NN]` shows a QoS
configuration to probe. `[00]` is a run with iocost off which is always
included and used as the baseline to compare other QoS runs against. Each
following line prints the QoS configuration to probe which is composed by
applying the specified overrides on top of the baseline QoS parameters - the
items in brackets are the ones overridden.

The `-` or `+` sign in front indicates whether the specific configuration is
actually scheduled to run. `iocost-qos` can take many hours to finish and
implements incremental completion. Here, `result.json` already contains the
results for the baseline and vrate at 100% and will be skipped.

`[05]` is marked `-s` indicating that the configuration is excluded because
the configuration would throttle the device too much.

The last line is summarizing that there are three bench sets to run.


## Reading Results

For each run, the QoS configuration is printed, followed by the `storage`
and `protection:mem-hog-tune` sub-bench results and then the result section
which ends with:

```
vrate: p00=60.00 p01=60.00 p10=60.00 p25=60.00 p50=60.00 p75=60.00 p90=60.00
       p99=60.00 p100=73.73 pmean=60.05 pstdev=0.75

QoS result: MOF=1.451@16(1.020x) vrate=60.05:0.75 missing=4.12%
            aMOF=1.451@16(1.020x) isol-01=97.45% lat_imp=0.79%:0.84 work_csv=73.55%
```

`vrate` line is showing the distribution of vrate across the run. Here, the
target min and max were 60%.

`QoS result` is showing the MOF and aMOF along with vrate mean and standard
deviation and other statistics from `storage` and `protection` sub-benches.
Here, `MOF=1.451@16(1.020x)` is indicating that MOF was 1.451 at memory
profile of 16G which was 1.020 times the baseline MOF with iocost off.

At the end, `iocost-qos` prints the summary of all runs:

```
Summary
=======

[00] QoS: iocost=off mem_profile=16
[01] QoS: rpct=99.00 rlat=3759 wpct=99.00 wlat=1382 [min=100.00] [max=100.00]
[02] QoS: rpct=99.00 rlat=3759 wpct=99.00 wlat=1382 [min=80.00] [max=80.00]
[03] QoS: rpct=99.00 rlat=3759 wpct=99.00 wlat=1382 [min=60.00] [max=60.00]
[04] QoS: rpct=99.00 rlat=3759 wpct=99.00 wlat=1382 [min=40.00] [max=40.00]
[05] QoS: rpct=99.00 rlat=3759 wpct=99.00 wlat=1382 [min=20.00] [max=20.00]

         MOF     aMOF  isol-01%       lat-imp%  work-csv%  missing%
[00]   1.423     FAIL         -       -:     -          -       1.9
[01]   1.522    1.393      90.3     3.6:   7.1       49.2       2.2
[02]   1.477    1.224      92.5     0.3:   2.1       56.5       2.4
[03]   1.451    1.451      97.5     0.8:   0.8       73.6       4.1
[04]   1.132    1.132      98.2     0.4:   0.5       30.9       1.8
[05]   1.146    1.118      98.6     0.4:   0.5       45.3       1.8

RLAT               p50                p90                p99                max
[00]  343u: 1.3m/67.5m   1.7m: 3.5m/95.5m   5.6m: 8.7m/ 150m  14.3m:21.6m/ 550m
[01]  190u: 105u/ 1.5m   867u: 513u/ 7.5m   3.6m: 2.7m/55.5m  11.1m:16.8m/ 450m
[02]  157u:79.8u/ 975u   620u: 495u/ 7.5m   2.9m: 2.0m/21.5m   9.1m: 6.1m/ 150m
[03]  170u:73.8u/ 855u   665u: 558u/ 4.5m   2.8m: 2.1m/15.5m   8.6m: 4.5m/32.5m
[04]  162u:35.4u/ 685u   466u: 326u/ 6.5m   2.3m: 2.1m/42.5m   7.6m: 5.3m/ 150m
[05]  137u:50.5u/ 1.5m   348u: 806u/31.5m   1.7m: 2.3m/49.5m   5.9m: 5.3m/ 150m

WLAT               p50                p90                p99                max
[00]  666u: 4.7m/93.5m   3.1m:12.2m/96.5m   6.0m:18.3m/ 250m  10.9m:33.8m/ 650m
[01] 35.1u:28.9u/ 415u   345u: 3.5m/95.5m   1.1m: 6.3m/ 150m   4.0m:26.1m/ 550m
[02]  113u: 2.4m/78.5m   387u: 3.3m/89.5m   948u: 5.5m/ 150m   2.7m:17.6m/ 550m
[03] 56.2u:48.5u/ 315u   501u: 2.1m/44.5m   1.2m: 4.1m/61.5m   2.5m: 5.4m/67.5m
[04] 85.3u: 1.2m/46.5m   350u: 2.4m/63.5m   656u: 3.9m/94.5m   1.9m: 9.8m/ 250m
[05] 27.7u:50.8u/ 1.5m   247u: 1.1m/21.5m   411u: 1.5m/31.5m   1.2m: 4.7m/ 150m
```

The firs table is showing the configurations probed, the second the results
for each configuration, the third and fourth tables the read and write
latencies.

Here, on the `RLAT` table, `[03]` row, `p99` column is `2.8m: 2.1m/15.5m`
which indicates that the `p99` read latencies for vrate of 60 were measured
to have the average of 2.1 millisecs, the standard deviation of 2.1
millisecs, and maximum of 15.5 millisecs.


## Properties
### First group properties (applies to all sub-runs)

##### `vrate-min` (float, default: 0.0)

See `vrate-intvs`.

##### `vrate-max` (float, default: 100.0)

See `vrate-intvs`.

##### `vrate-intvs` (integer, default: 0)

If non-zero, vrates between `vrate-min` and `vrate-max` are probed in
`vrate-intvs` steps. See the above overview for an example. This interval
based probing can be used together with direct QoS specifications.

##### `dither` (none or float)

Enables interval dithering. This offsets the `vrate-intvs` intervals by a
random amount so that the intervals don't fall on the exact same boundaries
when running this benchmark multiple times so that fine granularity data can
be obtained from multiple coarse interval runs.

When `dither` is specified without a value, the maximum dither distance is
half of interval width. Specifying a value overrides the maximum dither
distance.

##### `storage-base-loops` (integer, default: 3)

`loops` for the baseline (`iocost=off`) `storage` sub-bench.

##### `storage-loops` (integer, default: 1)

`loops` for QoS `storage` sub-benches.

##### `isol-pct` (percentile, default: 01)

`isol-pct` for protection sub-bench.

##### `isol-thr` (fraction, default: 0.9)

`isol-thr` for protection sub-bench.

##### `retries` (integer, default: 1)

The number of times to retry `storage` sub-bench after a failure.

##### `allow-fail` (boolean, default: false)

If the `storage` sub-bench fails for a QoS configuration, skip it instead of
aborting the whole benchmark.

##### `ignore-min-perf` (boolean, default: false)

`iocost-qos` automatically skips QoS configurations which result in too low
bandwidth for reliable operation. This property forces benchmarking of all
specified QoS configurations.


### Second+ group properties

Each group specifies the QoS configuration to probe which is composed by
applying the specified overrides on top of the active QoS parameters.

##### `rpct` and `wpct` (float)

Read and write latency percentiles for dynamic vrate adjustments. See `rlat`
and `wlat`. If 0, the latency doesn't affect vrate.

##### `rlat` and `wlat` (integer)

Read latency threshold in milliseconds for dynamic vrate adjustments. If
`rpct`th percentile read completion latency rises above `rlat`, the device
is considered saturated and vrate is adjusted downwards. The same for
writes.

##### `min` and `max` (float)

The minimum and maximum bounds for vrate adjustments. The value is in
percentage where 100.0 means no scaling of the model parameters. If `min` ==
`max`, vrate is fixed at the value.

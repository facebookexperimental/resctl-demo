#  `protection` benchmark

This benchmark is a collection of scenarios that test how well `rd-hashd`
can be protected against interferences. Currently, the following two
scenarios are implemented:

 * `mem-hog`: `rd-hashd` is stabilized at the target load level and then
   memory hog is started in `system.slice`. RPS is monitored until the
   memory hog dies or 5 mins have passed. See the "Cgroup and Resource
   Protection" section in `resctl-demo` for more background information.

 * `mem-hog-tune`: This scenario builds on top of the `mem-hog` scenario and
   determines the maximum memory footprint that can be protected
   sufficiently. `rd-hashd` is loaded to full and then the memory hog is
   started. If RPS drops below the target threshold, the run is considered a
   failure. The process is repeated with `rd-hashd`'s memory footprint
   reduced until RPS can be protected sufficiently.

This benchmark accepts multiple property groups and each group after the
first one specifies the scenario to run. For example,

```
$ resctl-bench -r result.json run \
  protection::scenario=mem-hog,load=1.0:scenario=mem-hog,load=0.8
```

would first run the `mem-hog` scenarios with the `rd-hashd` target load
level at 100% and then the same scenario with the target load level at 80%,
which happens to be what `protection` benchmark runs if no scenario is
specified.

This benchmark also supports rstat. Format the result with `--rstat`s to see
detailed resource statistics.


## Reading Results

### `mem-hog` Results

Here's a snippet from a `mem-hog` result:

```
Isolation and Request Latency Impact Distributions:

              min   p01   p05   p10   p25   p50   p75   p90   p95   p99   max  mean stdev
isol%           0  9.10 38.05 44.55 60.90 98.25 100.0 100.0 100.0 100.0 100.0 80.81 23.76
lat-imp%        0     0     0     0     0  0.91  4.50 111.9 271.8 867.5  2848 48.72 205.3

Result: isol=80.81:23.76% lat_imp=48.72%:205.3 work_csv=52.89% missing=0.11%
```

##### `isol%` distribution

This is the percentiles, mean and standard deviation of the isolation factor
which is measured every second and defined as:

```
MEASURED_RPS / TARGET_RPS
```

Here, `isol-01` (`isol%` row, `p01` column) is 9.10%, which can be
interpreted as:

> Every 100 seconds, the RPS dropped down to 1/10th of the target.

##### `lat-imp%` distribution

This is the percentiles, mean and standard deviation of the latency impact
which is measured every second and defined as the ratio which the request
response latencies increased by.

```
(MEASURED_LATENCY - BASELINE_LATENCY) / BASELINE_LATENCY
```

Here, `lat-imp-90` (`lat-imp%` row, `p90` column) is 111.9%, which can be
interpreted as:

> Every 10 seconds, the latency spikes over twice the baseline.

##### The `Result` block

* `isol` is the mean and standard deviation of `isol%`.

* `lat_imp` is the mean and standard deviation of `lat-imp%`.

* `work_csv` is work conservation and measures how much IO bandwidth the
  kernel was able to preserve while protecting against memory hog. The
  lossage is caused by inefficiency in the current implementation of
  anonymous memory throttling and doesn't reflect IO device characteristics.

* `missing` is the percentage of missing `rd-agent` report files. This is
  primarily interesting for debugging and everything is fine as long as it
  stays low single digit.


### `mem-hog-tune` Results

`mem-hog-tune` has an extra line in its `Result` block:

```
Isolation and Request Latency Impact Distributions:

              min   p01   p05   p10   p25   p50   p75   p90   p95   p99   max  mean stdev
isol%       97.60 97.60 98.90 99.20 99.40 99.85 100.0 100.0 100.0 100.0 100.0 99.64  0.48
lat-imp%        0     0  0.14  0.32  1.10  1.66  2.23  2.55  2.65  2.89  2.89  1.62  0.78

Result: isol=99.64:0.48% lat_imp=1.62%:0.78 work_csv=76.43% missing=1.75%
        hashd memory size 13.2G/13.6G can be protected at isol-01 >= 90.00%
```

By default, `mem-hog-tune` targets to find the memory footprint where
`isol-01` is higher than or equal to 90%. Here, it's reporting that the
original memory footprint was 13.6G and the protection target could be
reached after reducing it to 13.2G. From the distribution table, we can see
that `isol-01` was 97.6%.

If a memory footprint size which can be sufficiently protected can't be
found, the result looks like the following:

```
Isolation and Request Latency Impact Distributions:

              min   p01   p05   p10   p25   p50   p75   p90   p95   p99   max  mean stdev
isol%       49.40 49.40 80.60 95.60 98.75 100.0 100.0 100.0 100.0 100.0 100.0 97.17  9.00
lat-imp%        0     0     0     0  0.15  1.18  4.42 11.88 15.15 81.69 120.9  5.15 14.17

Result: isol=97.17:9.00% lat_imp=5.15%:14.17 work_csv=100.0% missing=1.23%
        Failed to find size to keep isol-01 above 90.00% in [9.6G, 16.2G]
```

Here, `mem-hog-tune` probed memory footprint sizes from 16.2G down to 9.6G
but couldn't find a size which could be successfully protected. The table
shows that `isol-01` was 49.4% on the final probe for 9.6G.


## Properties

`protection` doesn't yet have any first group properties.

### `mem-hog` Properties

##### `loops` (integer, default: 2)

The number of repetitions.

##### `load` (fraction, default: 1.0)

The target load level of `rd-hashd`. 1.0 or 100% indicates full load.

##### `speed` (default: 2x)

The memory hog growth speed expressed in relative terms to the maximum IO
device write speed according to the iocost model. Should be one of 10%, 25%,
50%, 1x or 2x.


### `mem-hog-tune` Properties

##### `load` (fraction, default: 1.0)

The target load level of `rd-hashd`. 1.0 or 100% indicates full load.

##### `speed`  (default: 2x)

The memory hog growth speed expressed in relative terms to the maximum IO
device write speed according to the iocost model. Should be one of `10%`,
`25%`, `50%`, `1x` or `2x`.

##### `size-min` (size)

The minimum `rd-hashd` memory footprint to probe. Must be specified.

##### `size-max` (size)

The maximum `rd-hashd` memory footprint to probe. Must be specified.

##### `intvs` (integer, default: 10)

The number of intervals to probe. Probing starts at `size-max` and decreases
by `(size-max - size-min) / intvs` until the size reaches `size-min`.

##### `isol-pct` (percentile, default: 01)

The isolation factor percentile to use when deciding protection success. The
value should match one of the percentile labels in the `isol%` distribution
table.

##### `isol-thr` (fraction, default: 0.9)

The isolation factor threshold to use when deciding protection success. The
`isol-pct`th isolation factor percentile should equal or be greater than
this value.

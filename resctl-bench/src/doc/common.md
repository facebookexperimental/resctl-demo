# Overview, Common Concepts and Options

When a system is under resource contention, various operating system
components and applications interact in complex ways. The interactions can't
easily be captured in synthetic per-component benchmarks making it difficult
to evaluate how the hardware and operating system would perform under such
conditions. resctl-bench solves the problem by exercising the whole system
with realistic workloads and analyzing system and workload behaviors.

Many of benchmarks implemented in resctl-bench have detailed explanations in
`resctl-demo`. Give it a try:

  https://github.com/facebookexperimental/resctl-demo


## Common Concepts

### rd-hashd

`rd-hashd` is a simulated latency-sensitive request-servicing workload with
realistic system resource usage profile and contention responses. Its page
cache and heap access patterns follow normal distributions and the load
level is regulated by both the target RPS and maximum response latency. The
default parameters are tuned so that resource-wise the behavior is rough an
approximation of a popular FB production workload.

While one workload can't possibly capture the many ways that systems are
used, `rd-hashd`'s behaviors and requirements fall where many
human-interactive and machine-saturating workloads' would.

`rd-hashd` has its own sizing benchmark mode where it tries to figure out
the parameters to saturate all of CPU, memory and IO. It finds the maximum
RPS that the CPUs can churn out and then figures out the maximum page cache
and heap footprints that the memory and IO device can service.
`resctl-bench` often uses this benchmark mode, usually with the CPU part
faked, to evaluate IO devices.

For more details: `rd-hashd --help`


### Memory Offloading and Profile

Not all memory areas are equally hot. If the IO device is performant enough,
the tail-end of the access distribution can be offloaded without violating
latency requirements. Modern SSDs, even the mainstream ones, can serve this
role in the [memory
hierarchy](https://en.wikipedia.org/wiki/Memory_hierarchy) quite effectively
by offloading page cache to filesystem and heap to swap.

This memory-offloading usage is critical not only because it makes a much
more efficient use of RAM but also because this is what happens when the
system is contended for memory and IO. If the system can't effectively
handle memory offloading, the application's quality-of-service will be
severely impacted under resource pressure, which lowers service reliability
and forces under-utilization of the systems.

`resctl-bench` uses the amount of `rd-hashd`'s memory footprint that can be
offloadedto the underlying IO device as the primary IO performance metric.
It's usually reported as MOF (Memory Offloading Factor) whose definition is:

```
  SUPPORTABLE_MEMORY_FOOTPRINT / MEMORY_SIZE
```

For example, the MOF of 1.2 means that the IO device can offload 20% of
available memory without violating service requirements. Note that both
bandwidth and latency contribute to MOF - bandwidth is meaningful only when
quick enough to meet the latency requirements.

The IO usage of memory offloading are influenced by the amount of available
memory. To ensure that bench results including MOFs are comparable across
different setups, `resctl-bench` uses memory balloon to constrain the amount
of available memory to a common value. This is called `mem_profile` (memory
profile) which is in gigabytes and always a power-of-two. The default
`mem_profile` is 16 and can be changed with the `--mem-profile` option.

The `mem_profile` of 16 tries to emulate a machine with 16G of memory. As
not all memory would be available for the workload, the net amount available
for workload is called `mem_share`. `rd-hashd` sizing benchmark is run with
a bit less memory to account for the raised memory requirement for longer
non-bench runs. This amount is called `mem_target`.

`resctl-bench` needs to know how much memory is actually available to
implement `mem_profile` and automatically tries to estimate on demand. If
the available memory amount is already known (e.g. from the previous
invocation), `--mem-avail` can be used to skip this step. Some benchmarks
(`storage` and its super benchmarks such as `iocost-qos` and `iocost-tune`)
can detect incorrect `mem_avail` and retry automatically. Those benchmarks
may fail if the amount of available memory keeps fluctuating.


## Running Benchmarks

### The Result File and Incremental Completion

A benchmark run may take a long time and it is often useful to string up a
series of benchmarks - e.g. run `iocost-params` and `hashd-params` to
determine the basic parameters and then `iocost-qos`. While `resctl-bench`
strives for reliability, it is a set of whole system benchmarks which keep
pushing the system to its limits for extended periods of time. Something,
even if not the benchmark itself, can fail once in a while.

`resctl-bench` ensures forward-progress by incrementally updating benchmark
results as they complete. The following command specifies the above three
benchmark sequence. Note that `iocost-qos` will automatically schedule the
two prerequisite benchmarks if the needed parameters are missing. Here,
they're specified explicitly for demonstration purposes.

```
 $ resctl-bench -r result.json run iocost-params hashd-params iocost-qos
```

Let's say the first two benchmarks completed without a hitch but the system
crashed when it was halfway through the `iocost-qos` benchmark. If you
re-run the same command after the system comes back, the following will
happen:

* `resctl-bench` recognizes that `result.json` already contains the results
  from `iocost-params` and `hashd-params`, outputs the summary and applies
  the result parameters without running the benchmarks again.

* Because `iocost-qos` benchmark can easily take multiple hours, it
  implements incremental completion and keeps the result file updated as the
  benchmark progresses. `iocost-qos` will fast-forward to the last
  checkpoint saved in `result.json` and continue from there.

The incremental operation means that the existing result files have
significant effects on how `resctl-bench` behaves. If `resctl-bench` is
behaving in an unexpected way or you want to restart a benchmark sequence
with a clean slate, specify a different result file or delete the existing
one.

The result file is in json. The `summary` and `format` subcommands format
the content into human readable outputs. On the completion of each
benchmark, the result summary is printed out which can be reproduced with
the following:

```
 $ resctl-bench -r result.json summary
```

For more detailed output:

```
 $ resctl-bench -r result.json format
```

By default, all benchmark results in the result file are printed out. You
can select the target benchmarks using the same syntax as the `run`
subcommand. To only view the result of the `iocost-qos` benchmark:

```
 $ resctl-bench -r result.json format iocost-tune
```


### The `run`, `study`, `solve` and `format` Stages

A benchmark is executed in the following four stages, each of which can be
triggered by the matching subcommand. When a stage is triggered, all the
subsequent stages are triggered together.


#### `run`

The actual execution of the benchmark. The system is configured and the
system requirements are verified and recorded. During and after the
benchmark, information is collected and put into the `record` section of the
result file.

A `record` is supposed to contain the minimum amount of information needed
to analyze the benchmark. e.g. it may just contain the relevant time ranges
so that the following `study` stage can analyze the agent report files in
`/var/lib/resctl-demo/report.d`.


#### `study`

This optional stage analyzes what happened during the `run` stage and
produces the `result` from the `record` and the agent report files. It does
not change system configurations or care about system requirements.

The separation of the `run` and `study` stages are useful for debugging and
development as it allows the bulk of data processing to be repeated without
re-running the entire benchmark which may take multiple hours.

`study` is often used with the `pack` subcommand which creates a tarball
containing the result file and the relevant report files:

```
$ resctl-bench -r result.json pack
```

The resulting tarball can be extracted on any machine and studied:

```
$ tar xvf output.tar.gz
$ resctl-bench -r result.json study
```

The above usage is recommended as the report files in their original
location expire after some time. If you want to study the report files in
place:

```
$ resctl-bench -r result.json study \
  --reports /var/lib/resctl-demo/report.d study
```


#### `solve`

This optional stage post-processes the existing `record` and `result` and
updates the latter. Note that this stage can only access what's inside the
result file and isn't allowed to access the reports or any system
information.

For example, `iocost-tune` uses the `solve` stage to calculate the QoS
solutions from the compiled experiment results so that users can calculate
custom solutions using only the result file.


#### `format / summary`

This stage formats the benchmark result into a human readable form. The
output is usually plain text but some benchmarks support different output
formats (e.g. pdf).

The `summary` subcommand is a flavor of the `format` stage which generates
an abbreviated output. This is what gets printed after each benchmark
completion.


### `run` and `format` Subcommand Properties

The `run` and `format` subcommands may take zero, one or multiple property
groups. Here's a `run` example:

```
 $ resctl-bench -r result.json run \
   iocost-qos:id=qos-0,storage-base-loops=1:min=100,max=100:min=75,max=75:min=50,max=50
```

We're running an `iocost-qos` benchmark and it has four property groups
delineated with colons. The properties in the first group apply to the whole
run.

##### `id=qos-0`

Specifies the identifier of the run. This is useful when there are multiple
runs of the same benchmark type. Here, we're naming the benchmark `qos-0`.

This is one of several properties which are available for all bench types.

##### `storage-base-loops=1`

This is an `iocost-qos` specific property configuring the repetition count
of the `storage` sub-bench base runs. The default is 3 but we want a quick
run and are setting it to 1. See the doc page of each bench type for details
on the supported properties.

While there is no strict rule on how the extra property groups should be
used, they usually specify a stage in multi-stage benchmarks. Here, we're
telling `iocost-qos` to probe three different QoS settings - vrate at 100,
75 and lastly 50. An empty property group can be specified with two
consecutive colons:

```
 $ resctl-bench -r result.json run iocost-qos:::min=75,max=75:min=50,max=50
```

The triple colons indicate that the first two property groups are empty and
the command will run an `iocost-qos` benchmark with the default parameters
to probe three QoS settings:

1. Default without any overrides

2. vrate at 75%

3. vrate at 50%

Similarly, the `format` subcommand may accept properties:

```
 $ resctl-bench -r result.json format iocost-tune:pdf=output.pdf
```

The above command tells `iocost-tune` to generate an output pdf file instead
of producing text output on stdout.


## Common Command Options and Bench Properties

### Common Command Options

Here are explanations on select common command options:

##### `--dir` and `--dev`

By default, `resctl-bench` uses `/var/lib/resctl-demo` for its operation and
expects swaps to be on the same IO device, which it auto-probes. `--dir` can
be used to put the operation directory somewhere else and `--dev` overrides
the underlying IO device detection.

##### `--mem-profile` and `--mem-avail`

For memory-size dependent benchmarks, `--mem-profile` can be used to select
a custom memory profile other than the default of 16. The memory profiles
must be identical for the results to be comparable. You can also turn of
memory profile and run the benchmarks at the machine size.

`resctl-bench` needs to probe how much memory is available when setting up
memory profiles which can be time consuming. If the available memory size is
already known from previous runs, `--mem-avail` can be used to bypass this
step.

##### `--iocost-from-sys` and `--iocost-qos`

Unless overridden, `resctl-bench` uses the `iocost` parameters from
`/var/lib/resctl-demo/bench.json`, which can be updated by `resctl-demo` or
running `iocost-params` benchmark with the `commit` property. If you want to
use the currently configured parameters instead, use `--iocost-from-sys`.
Note that this won't update `/var/lib/resctl-demo/bench.json`.

You can also manually override the iocost QoS parameters with
`--iocost-qos`. For example, `--iocost-qos min=75,max=75` will confine vrate
to 75%.

##### `--swappiness`

`resctl-bench` configures the default swappiness of 60 while running
benchmarks unless overridden by this option.

##### `--force`

When the sytstem can't be configured correctly or some dependencies are
missing, `resctl-bench` prints out error messages and exits. This option
forces `resctl-bench` to continue.


### Common Bench Properties

All common properties are for the first property group.

##### `id`

This gives the benchmark an optional identifier which helps with
identification if there are multiple instances of the same bench type in the
series. `resctl-bench` doesn't mind multiple instances of the same bench
type without IDs:

```
$ resctl-bench -r result.json run \
  iocost-qos \
  iocost-qos::min=50,max=50:min=75,max=75
```

However, if IDs are specified, they must be unique for the bench type.

In addition to helping differntiating bench instances, IDs are used to group
source results when merging with `--by-id` specified.

##### `passive`

`resctl-bench` verifies and changes system configurations so that the
benchmarks can measure the system behavior in a controlled and expected
manner. The configurations that `resctl-bench` controls include but are not
limited to cgroup hierarchy and controllers, IO device elevator and wbt,
sysctl knobs, and btrfs mount options.

While the configuration enforcement helps running benchmarks reliably and
conveniently, it gets in the way when trying to test custom configurations.
The `passive` property can be used to tell `resctl-bench` to accept the
system configurations as-are. The following values are accepted:

* `ALL`: `resctl-bench` won't change any system configurations.

* `all`: Only memory protection for `hostcritical.slice` is enforced.

* `cpu`: Don't touch CPU controller configurations.

* `mem`: Don't touch memory controller and other memory related
  configurations.

* `fs`: Don't touch filesystem related configurations.

* `io`: Don't touch IO controller and other IO related configurations.

* `oomd`: Don't touch existing `oomd` or `earlyoom` instance and don't start
  one either.

* `none`: Clear the passive settings.

Multiple values can be specified by delineating them with `/`:

```
$ resctl-bench -r result.json run iocost-qos:passive=mem/io
```

##### `apply` and `commit`

These two boolean properties are available in benchmarks that produce either
iocost or hashd parameters. `apply`, when true, makes the benchmark apply
the result parameters to the subsequent benchmarks in the series. `commit`,
when true, makes the benchmark update `/var/lib/resctl-demo/bench.json` with
the result parameters. `commit` implies `apply`.

The parameters can be specified without value to indicate `true`. IOW, the
followings are equivalent:

```
$ resctl-bench -r result.json run storage:apply
$ resctl-bench -r result.json run storage:apply=true
```

Note that the properties default to `true` for some benchmarks
(`iocost-params` and `hashd-params`).


## Reading Benchmark Results

### Header

When formatted, each benchmark result starts with a header which looks like
the following:

```
[iocost-tune result] 2021-05-08 11:06:38 - 04:16:25

System info: kernel="5.12.0-work+"
             nr_cpus=16 memory=32.0G swap=16.4G swappiness=60
             mem_profile=16 (avail=30.1G share=12.0G target=11.0G)

IO info: dev=nvme0n1(259:5) model="Samsung SSD 970 PRO 512GB" size=477G
         iosched=mq-deadline wbt=off iocost=on other=off
         iocost model: rbps=2992129542 rseqiops=337745 rrandiops=370705
                       wbps=2232405244 wseqiops=260917 wrandiops=256225
         iocost QoS: rpct=95.00 rlat=11649 wpct=95.00 wlat=12681 min=8.83 max=8.83
```

The first line shows the bench type, ID if available, and time duration of
the run.

The system info block shows the basic system configuration - kernel version,
hardware configuration and the memory profile parameters.

The IO info block shows information on the IO device and IO related kernel
configurations - device model, IO scheduler, wbt status, IO controller
status and iocost parameters. Note that the iocost parameters are captured
at the beginning of the benchmark. For benchmarks which produce their own
parameters, the parameters in the header are not meaningful.

Additionally, if the benchmark was `--force`'d to run, the missed system
requirements will be printed as well.

### Nested IO Latency Distribution

For benchmarks which care about IO completion latencies,`resctl-bench`
reports IO them in a table which looks like the following:

```
  READ      min   p25   p50   p75   p90   p95   p99 p99.9   max   cum  mean  stdev
  min      5.0u  5.0u  5.0u 35.0u 45.0u 55.0u 75.0u  155u  165u  5.0u 21.1u  20.4u
  p01      5.0u 45.0u 75.0u 85.0u 95.0u 95.0u  115u  185u  205u  5.0u 66.1u  30.3u
  p05      5.0u 85.0u 85.0u 95.0u  105u  115u  185u  595u  725u  5.0u 90.3u  45.8u
  p10      5.0u 95.0u 95.0u  105u  125u  145u  315u  705u  975u  5.0u  106u  62.9u
  p25      5.0u  115u  125u  145u  205u  245u  795u  955u  985u  105u  146u  95.3u
  p50      5.0u  145u  195u  275u  425u  585u  1.5m  2.5m  2.5m  205u  256u   217u
  p75      5.0u  255u  395u  615u  995u  1.5m  2.5m  5.5m  7.5m  575u  554u   569u
  p90      5.0u  485u  815u  1.5m  2.5m  3.5m  5.5m 12.5m 14.5m  2.5m  1.1m   1.2m
  p95      5.0u  715u  1.5m  2.5m  3.5m  4.5m  8.5m 19.5m 21.5m  4.5m  1.8m   1.8m
  p99      5.0u  1.5m  2.5m  4.5m  7.5m  9.5m 19.5m 56.5m 72.5m 10.5m  3.7m   4.2m
  p99.9   95.0u  2.5m  4.5m  6.5m  9.5m 13.5m 40.5m 71.5m 93.5m 28.5m  5.6m   6.9m
  p99.99   105u  3.5m  5.5m  8.5m 11.5m 15.5m 43.5m 74.5m  250m 59.5m  6.9m   8.9m
  p99.999  115u  4.5m  6.5m  9.5m 12.5m 17.5m 45.5m  150m  350m 84.5m  7.9m  10.5m
  max      125u  5.5m  7.5m 10.5m 14.5m 18.5m 49.5m  250m  450m  250m  9.1m  13.1m
```

The `cum`ulative column shows the usual overall latency percentiles. For
example, in the above table, `p99-cum` (`p99` row, `cum` column) is 10.5m
indicating that the 99th percentile of read completion latencies for the
whole benchmark was 10.5 milliseconds. While this already gives some
insight, it can't distinguish, for example, devices which stall out most
requests in short bursts from the usual spread-out long-tail high latency
events even though the former is a lot more disruptive.

`resctl-bench` calculates the IO completion latency percentiles every second
and then the distribution of them over the whole run. In the above, `p50-99`

- the `p50` row, `p99` column - is 1.5m, indicating that in one out of 100
  1s periods, the median latency is gonna be as high as 1.5 milliseconds.

Similarly, `pNN-mean` and `pNN-stdev` indicate the geometric average and
standard deviation of 1s NN'th percentile completion latencies over the
duration of the benchmark.

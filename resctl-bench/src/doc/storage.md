`storage` benchmark
===================

Uses `rd-hashd`'s sizing benchmark to measure the performance of the storage
device.

`rd-hashd` benchmark sizes all three major resources - CPU, memory and IO.
This benchmark uses `--bench-fake-cpu-load` to ignore CPU differences and
memory profile to ignore memory size differences so that the result is only
dependent on the IO device.

With CPU and memory usages equalized, the supportable memory footprint is
determined by how much of the working set can be offloaded to the IO device
without violating the target response latency. For more details on memory
offloading, consult the common doc and resctl-demo.

This latency-limited bandwidth measurement is encoded as MOF (Memory
Offloading Factor) which is defined as:

```
SUPPORTABLE_MEMORY_FOOTPRINT / PHYSICAL_MEMORY_USED
```

For memory offloading, the IO accesses are determined by how memory is
accessed and how kernel memory management manages the memory. Thus, MOFs are
comparable only when the available memory sizes are identical. To avoid
confusion, MOFs are always indicated with the memory profile used for the
benchmark.

For an example, `MOF@16 = 1.3` indicates that the IO device could offload
30% of working set on 16G memory profile. 16G memory profile has 11G
available for the workload, so `rd-hashd` could run with the memory
footprint of 14.3G without violating the latency requirements.

Because the `rd-hashd` benchmark exercises the tall stack spanning the IO
device and kernel filesystem, memory and IO subsystems, there can be
variances in the system behavior and results. The `storage` benchmark
manages the variability with multiple benchmark runs and by detecting
available memory fluctuations and retrying adaptively.

If `apply` or `commit` is `true`, this benchmark will generate `rd-hashd`
parameters based on the final MOF result. Note that this parameter set will
always have `fake-cpu-load` set.

This benchmark also supports rstat. Format the result with `--rstat`s to see
detailed resource statistics.


Reading Results
===============

In the `storage` output:

```
IO BPS: read_final=332M write_final=40.6M read_all=300M write_all=44.0M

Memory offloading: factor=1.559@16 usage_mean/stdev=11.1G/36.5M size_mean/stdev=17.3G/1.7G
```

The `IO BPS` line shows the measured read and write bandwidths. The `_final`
suffixed ones are measurements from the final stages of the benchmarks where
`rd-hashd` was running with the final probed sizes. The `_all` ones are for
the entire benchmark duration.

In the `Memory offloading` line, `factor` is the `MOF`. `usage_mean/stdev`
are the average and standard deviation of `rd-hashd`'s memory usages over
the `loops` iterations. `size_mean/stdev` are the same of `rd-hashd`'s
memory footprints. The `factor` is `size_mean` divided by `usage_mean`.


Properties
==========

#### `apply` (bool, default: false)

If true, apply the determined parameters to the subsequent benchmarks.

#### `commit` (bool, default: false)

If true, commit the determined parameters to
`/var/lib/resctl-demo/bench.json`. Implies `apply`.

#### `loops` (integer, default: 3)

The number of `rd-hashd` benchmark iterations to run. The final result is
averaged over the iterations.

#### `rps-max` (integer, default: 2000)

Configure the max RPS when faking cpu load.

#### `hash-size` (size, default: see `rd-hashd --help`)

Configure the average number of bytes hashed per request.

#### `chunk-pages` (integer, default: see `rd-hashd --help`)

Configure the number of pages to be accessed together.

#### `log-bps` (size, default: see `rd-hashd --help` )

Configure how many log bytes `rd-hashd` will generate per second.

#### `mem-avail-err-max` (fraction, default: 0.1)

If the memory used by `rd-hashd` is off by more than this ratio, declare the
run to be invalid and retry.

#### `mem-avail-inner-retries` (integer, 2)

The number of times to retry without re-evaluating the amount of available
memory.

#### `mem-avail-outer-retries` (integer, 2)

The number of times to retry after re-evaluating the amount of available
memory.

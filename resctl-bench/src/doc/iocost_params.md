# `iocost-params` Benchmark

This is a wrapper around the naive fio based benchmark available in the
kernel tree and determines the iocost model and QoS parameters.


## Properties

#### `apply` (bool, default: true)

If true, apply the determined parameters to the subsequent benchmarks.

#### `commit` (bool, default: true)

If true, commit the determined parameters to
`/var/lib/resctl-demo/bench.json`. Implies `apply`.


## Limitations

The benchmark runs six simple benchmarks to determine the six iocost model
parameters and then another two to determine the QoS latency targets. The
assumption that this benchmark makes - that the performance measured with
six homogeneous benchmarks would combine linearly with each other - is too
naive and often leads to overly optimistic model parameters and the QoS part
is too simplistic to compensate.

While far from being perfect, the resulting parameters already behave
substantially better than the generic default parameters and can serve as
the basis for further benchmarks and parameter tuning.

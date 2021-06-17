# `hashd-params` Benchmark

This is a wrapper around `rd-hashd`'s benchmark mode which tries to saturate
all three local resources - CPU, memory and IO. The bench result parameters
include hashing size, max RPS and memory footprint and are recorded in
`/var/lib/resctl-demo/bench.json` if `commit` is `true`.


## Properties

#### `apply` (bool, default: `true`)

If `true`, apply the determined parameters to the subsequent benchmarks.

#### `commit` (bool, default: `true`)

If `true`, commit the determined parameters to
`/var/lib/resctl-demo/bench.json`. Implies `apply`.

#### `fake-cpu-load` (bool, default: `false`)

If `true`, use short sleeps and generate bogus hash values instead of
actually burning CPU cycles. This allows saturated memory and IO subsystem
benchmarking independent of CPU performance. Useful when IO devices need to
be compared across different machines and thus used by storage focused
benchmarks.

#### `rps-max` (integer, default: `2000` when `fake-cpu-load`)

Configure the max RPS when faking cpu load.

#### `hash-size` (size, default: see `rd-hashd --help`)

Configure the average number of bytes hashed per request.

#### `chunk-pages` (integer, default: see `rd-hashd --help`)

Configure the number of pages to be accessed together.

#### `log-bps` (size, default: see `rd-hashd --help` )

Configure how many log bytes `rd-hashd` will generate per second.

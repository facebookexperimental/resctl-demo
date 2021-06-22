
# Resource Control Demo Hash Daemon

`rd-hashd` is a workload simulator for `resctl-demo` and `resctl-bench`. Its
primary goal is simulating a latency-senstive and throttleable primary
workload which can saturate the machine in all local resources.

Imagine a latency-sensitive user-request-servicing application which is load
balanced and configured to use all available resources of the machine under
full load. Under nominal load, it'd consume smaller amounts of resources and
show tighter latency profile. As load gets close to full, it'll consume most
of the machine and the latencies would increase but, hopefully, stay within
the target envelope. If the application gets stalled for whatever reasons
including resource conflicts, it'd experience latency spikes and the load
balancer would allocate it less requests until it can catch up.

`rd-hashd` simulates such workload in a self-contained manner. It sets up
test files and memory heap with random contents and keeps calculating SHA1s
using multiple worker threads. The concurrency level is modulated so that
the RPS converges on the target while not exceeding the latency target. The
RPS and latency targets can be dynamically modified. The memory access
patterns follow normal distributions, small random sleeps are injected to
emulate network interactions, and it also generates log writes.

Many aspects of `rd-hashd`'s resource consumption behaviors are
configureable and are tuned, by default, to behave similarly to a popular
Facebook web workload, especially under memory and IO contentions.

`rd-hashd` is used as the primary workload for both `resctl-demo` and
`resctl-bench`. While a single workload cannot possibly represent the many
different ways systems are used, it can successfully capture many aspects of
typical human-interactive workloads, reproduce a wide variety of resource
conflict issues and verify their remedies, and be used as an approximate
measure in evaluating hardware and operating system behaviors.

While `rd-hashd` is usually used a part of `resctl-demo` and `resctl-bench`,
it can be useful as a stand-alone pseudo workload too. For more information
on the containing projects, visit:

  https://github.com/facebookexperimental/resctl-demo


# Configuration, Report and Log Files

There are two configuration channels - command line arguments and runtime
parameters. The former can be specified as command line options or with the
`--args` file. The latter can be specified using the `--params` file and
dynamically updated while `rd-hashd` is running - just edit and save, the
changes will be applied immediately.

If the specified `--args` and/or `--params` files don't exist, they will be
created with the default values. Any configurations in the `--args` file can
be overridden on the command line and the file will be updated accordingly.
Note that only the arguments which are listed above `--args` in the help
message are saved. If `--params` is not specified, the defaults are used and
the parameters can't be updated while `rd-hashd` is running.

`rd-hashd` reports the current status in the optional `--report` file and
the hash results are saved in the optional log files in the `--log-dir`
directory.

The following will create the `--args` and `--params` files and exit.

```
$ rd-hashd --testfiles ~/rd-hashd/testfiles --args ~/rd-hashd/args.json \
  --params ~/rd-hashd/params.json --report ~/rd-hashd/report.json \
  --log-dir ~/rd-hashd/logs --interval 1 --prepare-config
```

Afterwards, `rd-hashd` can be run with the same configurations with:

```
  $ rd-hashd --args ~/rd-hashd/args.json
```


# Benchmarking

It is challenging to find the right parameters to maximize resource
utilization. To help determining the parameters, `--bench` runs a series of
tests and records the determined parameters in the specified `--args` and
`--params` files.

With the resulting parameters, `rd-hashd` should saturate CPU and memory and
use some amount of IO at the full load. RPS will be sensitive to memory and
thus IO availability and resource conflicts will lead to raised request
processing latencies and lowered RPS.

`--bench` may take over ten minutes and the system should be idle otherwise.
While it tries its best, due to long tail memory accesses and fluctuating
hardware IO performance characteristics, there is a low chance that the
resulting configuration might not be sustainable in extended runs. If
`rd-hashd` fails to keep CPU saturated and deviates significantly from the
target RPS, try lowering the runtime parameter `mem_frac`. If not enough IO
is being generated, try raising.

`--bench` preserves the existing parameters in the configuration files as
much as possible. If the benchmark behaves in an unexpected way, try
removing the configuration files to start from a clean slate.


# Usage Example

The following is an example workflow. It clears the existing configurations,
performs a benchmark to determine the parameters and then starts a normal
run.

```
  $ mkdir -p ~/rd-hashd
  $ rm -f ~/rd-hashd/*.json
  $ rd-hashd --args ~/rd-hashd/args.json --testfiles ~/rd-hashd/testfiles \
             --params ~/rd-hashd/params.json --report ~/rd-hashd/report.json \
             --log-dir ~/rd-hashd/logs --interval 1 --bench
  $ rd-hashd --args ~/rd-hashd/args.json
```

Resource Control Benchmarks
===========================

Resource control aims to control compute resource distribution to improve
reliability and utilization of a system. `resctl-bench` is a collection of
whole-system benchmarks to evaluate resource control and hardware behaviors
using realistic simulated workloads.

Comprehensive resource control involves the whole system - kernel subsystems
such as cgroup2, memory management, file system and block layer, userspace
system components and even the SSD. Furthermore, testing resource control
end-to-end requires scenarios involving realistic workloads and monitoring
their interactions. The combination makes benchmarking resource control
challenging and error-prone. It's easy to slip up on a configuration and
testing with real workloads can be tedious and unreliable.

`resctl-bench` encapsulates the whole process so that resource control
benchmarks can be performed easily and reliably. It verifies and updates
system configurations, reproduces resource contention scenarios with a
realistic latency-sensitive workload simulator and other secondary
workloads, analyzes the resulting system and workload behaviors, and
generates easily understandable reports.

`resctl-bench` is a part of `resctl-demo` suite, which gives a guided tour
of various resource control strategies using live scenarios built on the
same components. The benchmarks implemented in `resctl-bench` involve
concepts and components which are documented in `resctl-demo` in depth. For
more information on `resctl-demo`, visit:

  https://github.com/facebookexperimental/resctl-demo


Premade System Images
=====================

Comprehensive resource control has many requirements, some of which can be
difficult to configure on an existing system. `resctl-demo` provides premade
images to help getting started. Visit the following page for details:

  https://facebookmicrosites.github.io/resctl-demo-website

For other installation options, visit:

  https://github.com/facebookexperimental/resctl-demo


An Example Session
==================

Let's say we want to see how well iocost can protect `rd-hashd` and designed
a bench sequence like the following:

1. Run `hashd-params` to determine hashd parameters.
2. Run `protection` with iocost disabled to establish the baseline.
3. Run `iocost-params` to determine the iocost parameters.
4. Run `protection` with iocost enabled and compare the results.

Assuming the root device is `nvme0n1` with the device number `259:0`, this
maps to:

```
   $ echo '259:0 enable=0' > /sys/fs/cgroup/io.cost.qos
   $ echo 0 > /sys/block/nvme0n1/queue/wbt_lat_usec
   $ resctl-bench -r result.json run \
     hashd-params:passive=io \
     protection:id=iocost-off,passive=io \
     iocost-params \
     protection:id=iocost-on
```

* We want to run the first two benchmarks with iocost off. Turn it off
  manually and tell the first two to not touch IO related configurations.
  `wbt` is turned off too to stay consistent with `iocost` enabled
  configuration.
* To reduce confusion, we're marking the two `protection` runs with different IDs.

Here are the example outputs:

* Summary:  https://github.com/facebookexperimental/resctl-demo/blob/master/resctl-bench/examples/prot-iocost-off-on-summary.txt
* Format: https://github.com/facebookexperimental/resctl-demo/blob/master/resctl-bench/examples/prot-iocost-off-on-format.txt

Let's look at the result of the first benchmark - `hashd-params`.

```
   [hashd-params result] 2021-06-22 17:23:03 - 17:43:47

   System info: kernel="5.6.13-0_fbk16_5756_gdcbe47195163"
                nr_cpus=36 memory=63.9G swap=32.0G swappiness=60 zswap
                mem_profile=16 (avail=57.4G share=12.0G target=11.0G)
                passive=io

   IO info: dev=nvme0n1(259:0) model="WDC CL SN720 SDAQNTW-512G-1020" size=477G
            iosched=mq-deadline wbt=off iocost=off other=off

   Params: log_bps=1.0M

   Result: hash_size=1.2M rps_max=1029 mem_actual=16.1G chunk_pages=25
```

After the header, the following three blocks are showing the system and
bench configurations followed by the result.

* `passive=io`, so IO configurations were left as-are. We can see that
  `iocost` and `other` IO controllers were off.
* `zswap` is reported on. I forgot to turn it off. The subsequent benchmarks
  will automatically turn off `zswap` as they are storage focused
  benchmarks. It'd have been better if `zswap` were off here too but it
  shouldn't make much difference given that all data are incompressible and
  the primary goal of this bench is establishing the common measuring
  standard.
* The determined memory footprint is 16.1G, which is pretty good given that
  the amount of memory available to the benchmark - `mem_target` - was only
  11G.

Let's now take a look at the first next result. Partial header:

```
   [protection result] "iocost-off" 2021-06-22 19:13:37 - 19:30:25
   ...
   IO info: dev=nvme0n1(259:0) model="WDC CL SN720 SDAQNTW-512G-1020" size=477G
            iosched=mq-deadline wbt=off iocost=off other=off
```

shows that this is `protection` result with ID `iocost-off`. Skipping over
to the result:

```
   Memory Hog Summary
   ==================

   IO Latency: R p50=885u:3.7m/49.5m p90=4.7m:12.7m/150m p99=13.1m:25.1m/350m max=30.4m:65.4m/750m
               W p50=5.0m:16.3m/99.5m p90=17.6m:28.3m/250m p99=29.0m:38.8m/450m max=48.9m:87.0m/850m

   Isolation and Request Latency Impact Distributions:

                 min   p01   p05   p10   p25   p50   p75   p90   p95   p99   max  mean stdev
   isol%           0  0.49  1.65  2.24 13.12 50.90 72.52 82.12 88.56 100.0 100.0 45.50 30.72
   lat-imp%        0     0     0     0  4.69 17.00 40.54 75.06 121.9 380.3 882.5 39.42 81.53

   Result: isol=45.50:30.72% lat_imp=39.42%:81.53 work_csv=100.0% missing=0.26%
```

For brevity, let's just focus on the `isol=45.50:30.72%` on the last line,
which is indicating that the isolation factor - how well the RPS of the
`rd-hashd` could be protected against interferences from memory hogs -
averaged 45.5% with the standard deviation of 30.72%. Roughly speaking, our
main workload's RPS halved while the system was experiencing memory
shortage. For more information on the output format:

* `$ resctl-bench doc protection`
* https://github.com/facebookexperimental/resctl-demo/blob/master/resctl-bench/doc/protection.md

So, we now know that without `iocost`, the protection isn't great. The next
`iocost-params` benchmark determines the parameters so that we can enable
it. The result:

```
   iocost model: rbps=1348822120 rseqiops=235687 rrandiops=218614
                 wbps=601694170 wseqiops=133453 wrandiops=69308
   iocost QoS: rpct=95.00 rlat=19562 wpct=95.00 wlat=65667 min=60.00 max=100.00
```

`iocost-params` automatically applies the determined parameters for the
subsequent benchmarks. The QoS parameters determined here are very naive but
should do for our purpose. For determining more accurate QoS parameters and
evaluating storage devices comprehensively, see the `iocost-tune` benchmark.

Let's see whether the `protection` result is any better with `iocost` on:

```
   [protection result] "iocost-on" 2021-06-22 19:38:53 - 20:02:27
   ...
   IO info: dev=nvme0n1(259:0) model="WDC CL SN720 SDAQNTW-512G-1020" size=477G
            iosched=mq-deadline wbt=off iocost=on other=off
            iocost model: rbps=1348822120 rseqiops=235687 rrandiops=218614
                          wbps=601694170 wseqiops=133453 wrandiops=69308
            iocost QoS: rpct=95.00 rlat=19562 wpct=95.00 wlat=65667 min=60.00 max=100.00
```

The header confirms that we are testing the correct configuration. The
result:

```
   Memory Hog Summary
   ==================

   IO Latency: R p50=164u:42.2u/415u p90=915u:827u/17.5m p99=3.4m:4.5m/97.5m max=8.8m:10.3m/250m
               W p50=158u:1.7m/41.5m p90=2.3m:9.1m/95.5m p99=5.1m:14.3m/97.5m max=8.8m:21.7m/350m

   Isolation and Request Latency Impact Distributions:

                 min   p01   p05   p10   p25   p50   p75   p90   p95   p99   max  mean stdev
   isol%           0     0 88.34 90.57 93.78 97.30 100.0 100.0 100.0 100.0 100.0 95.18 11.06
   lat-imp%        0     0  0.96  2.20  3.79  6.49 10.22 15.63 18.32 29.55 263.0  8.14  9.99

   Result: isol=95.18:11.06% lat_imp=8.14%:9.99 work_csv=42.89% missing=0.21%
```

The isolation factor average is now 95.18% with the standard deviation of
11.06%, a significant improvement over 45.5% without `iocost`.

This example shows that testing resource control behaviors using scenarios
that exercise every layer of the tall stack realistically is easy and
reliable with `resctl-bench`. For more information, explore the doc pages:

* `$ resctl-bench doc --help`
* https://github.com/facebookexperimental/resctl-demo/tree/master/resctl-bench/doc

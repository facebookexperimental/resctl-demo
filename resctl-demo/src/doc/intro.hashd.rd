## Copyright (c) Facebook, Inc. and its affiliates.
%% id intro.hashd: rd-hashd Workload Simulator
%% reset prep
%% graph HashdA

*The rd-hashd Workload Simulator*\n
*===============================*

___*Overview*___

Imagine a latency-sensitive, user-request-servicing application, that's load
balanced, and configured to use all available resources of the machine under
full load.

Under nominal load, it would consume lower amounts of resources and show a
tighter latency profile. As load gets close to full, it'll consume most of
the machine. The latencies would increase but stay within a certain
envelope. If the application gets stalled for whatever reason including any
resource conflicts, it would experience a latency spike, and the load
balancer would allocate it less requests until it can catch up.

The above description fits many distributed latency-sensitive workloads in a
server fleet. Here are some common characteristics:

* Tail latency matters, but a single miss isn't going to ruin everything.

* Should strike a reasonable balance between throughput and latency.

* Should be load-balanced and respond to load-level changes.

* Should be able to ride out temporary system-level disruptions such as
  brief resource contention.

* Should have a combination of page cache and anonymous memory access patterns
  with hot and cold areas like any other application.

* Should have some write IOs for logs and other persistent data.

Benchmarks typically measure specific aspects of system performance, but the
challenge is to gauge what the aggregate behavior would be for an
application like above. While nothing is more accurate than using production
workloads directly, that's usually a cumbersome and challenging process.

rd-hashd simulates such workloads in a self-contained manner. It sets up
test files with random contents and keeps calculating SHA1s of different
parts using concurrent worker threads. The concurrency level is modulated so
that RPS converges on the target while not exceeding the latency limit. The
targets can be dynamically modified while rd-hashd is running. The workers
also sleep randomly, generate anonymous memory accesses, and write to the
log file.


___*Sizing and benchmark*___

We all want our workloads to utilize the machines as much as possible.
Toward that end, we spend considerable time and effort to tune the workload
so that, under full load, it saturates the machine just enough so that all
available resources are utilized while leaving a sufficient buffer to keep
the machine from falling apart when the maintenance cron job kicks in at
midnight.

To ease testing, rd-hashd can benchmark and size itself to automatically
determine the following, so that it can saturate all local resources:

* The mean hash size: So that each request consumes ~10ms worth of CPU
  cycles.

* The max RPS the machine can serve: The RPS at which all CPUs are fully
  utilized without any memory or IO contention.

* IO write bandwidth: This is scaled by adjusting the log padding size. This
  is set to 5% of the sequential write bandwidth of the IO device at the
  full load.

* Memory footprint: With the above parameters, CPU is fully saturated and IO
  is lightly loaded. rd-hashd finds the saturation point for memory and IO
  by bisecting for the memory footprint where the system struggles to
  service more than 90% of max RPS while maintaining 90th percentile
  completion latency under 100ms.

With the resulting configurations, rd-hashd should closely saturate CPU and
memory and use some amount of IO when running with the target p90 latency
100ms. Its memory (and thus IO) usage will be sensitive to RPS, so that any
stalls or resource shortages will lead to lowered RPS.

For both page cache and anonymous memory, the access pattern follows
truncated normal distribution. A fast IO device will be able to keep within
the latency target while servicing more reclaims and refaults: Thus, the
system as a whole will be able to serve a larger memory footprint. This
effect becomes apparent when running the benchmark in comparable machines
with different IO devices. You effectively gain more usable memory with a
performant IO device.

While the benchmark tries its best to deliver reliable results consistently,
it might get it wrong especially if the underlying IO device's performance
characteristics change significantly over time, which isn't too uncommon
with SSDs. If you see behaviors inconsistent with the scenarios later in the
demo, you may want to re-run the benchmark, or manually calibrate the
parameters. Read on for details.


___*Configuration*___

Many aspects of rd-hashd behavior can be configured. There are two types of
configurations - arguments which require rd-hashd to be restarted to apply,
and parameters which can be applied immediately while rd-hashd is running.

Once the benchmarks are complete, the arguments are stored in
/var/lib/resctl-demo/hashd-A/args.json. Here are some arguments which may be
interesting. The first two are useful if you want to push memory footprint
and page cache fraction higher than the defaults:

* size: Maximum memory footprint in bytes. This in combination with
  `file_max_frac` determines the amount of space used by testfiles. The
  actual memory footprint is determined by scaling this down with the
  `mem_frac` parameter. Defaults to 3 times the amount of system memory.

* file_max_frac: Maximum fraction of page cache out of memory footprint. 0.0
  means all of memory footprint is anonymous memory, 1.0 all page cache. The
  actual page cache fraction is determined by the `file_frac` parameter
  which is capped by this argument. Defaults to 0.25.

For a full explanation, see `rd-hashd --help`.

The runtime tunable parameters are in
/var/lib/resctl-demo/hashd-A/params.json which has full documentation at the
top. Editing the file changes rd-hashd's behavior immediately. However, some
parameters are overridden by bench and other resctl-demo operations.

The following parameters are determined by benchmark. Don't change them
manually:

* file_size_mean: Mean hash size. Determines how many CPU cycles each RPS
  consumes.

* rps_max: The maximum rps. rd-hashd isn't bound by this but uses it to
  calculate the current load level and scale operations, e.g., memory
  footprint, accordingly.

* mem_frac: Memory footprint scaling factor. rd-hashd will use
  `file_max_frac` * `mem_frac` bytes. This can be changed through the demo
  interface.

The following parameters are controlled by this demo program and should be
modified through the demo interface:

* rps_target: The target RPS.

* file_frac: Page cache proportion of memory footprint. Defaults to 0.15
  indicating 15% of memory footprint is page cache. Capped by
  `file_max_frac`.

* anon_write_frac: The proportion of anon accesses which are writes.

* log_bps: Log write BPS at maximum RPS.


___*Tuning the parameters*___

%NeedBench%***WARNING***: This section requires the benchmarks to be
complete. Please wait for them to finish and refresh this page by pressing
'r' before proceeding.

The only parameter which may need manually tuning is `mem_frac`.

resctl-demo reserves some memory for the rest of the system, %DflMemMargin%
by default, while running the benchmarks, as the system should be able to
service managerial workloads even while rd-hashd is fully loaded. Note that
the amount is reserved only during the benchmark. The memory is available to
rd-hashd and other applications once the benchmarks are complete.

The goal here is finding the `mem_frac` value where:

1. The system can reliably service rd-hashd at close to full RPS with the
   default amount of reserve memory set aside while moderately stressing the
   IO device with refaults.

2. The system can't reliably service rd-hashd at close to full RPS, and RPS
   starts falling due to resource contention if the reserve is pushed beyond
   twice the default reserve.

Let's first start hashd

%% on hashd                      : [ Start hashd ]

and set the load level to maximum.

%% knob hashd-load               : hashd load   :

Use the following slider to set the reserve amount close to %DflMemMargin%.
It doesn't have to be exact.

%% knob balloon                  : Reserve size :

RPS will ramp up close to 100%. It will take several minutes for the working
set to be established and possibly many more minutes to smoke out SSD
performance swings.

Check out the graph view by pressing 'g'. As the workload stabilizes, it
should be issuing a moderate amount of reads from refaults, say, 10-20% of
maximum read bandwidth. If you don't see any read IOs or the device is close
to saturation or overloaded, adjust the memory footprint using the following
slider.

%% knob hashd-mem                : hashd memory :

After making an adjustment, leave it alone for a while so that it can reach
a stable state.

Once verified, go back and push the reserve size to twice. The system should
show signs of distress soon. Keep adjusting until you're happy with the
behavior difference between the two reserve sizes.

The level of distress depends on the IO device and you may need to push the
reserve size further to see a drastic difference on high performance SSDs.
This can be offset by making the memory access pattern flatter by increasing
`file_addr_stdev_ratio` and `anon_addr_stdev_ratio` in the `params.json`
file.

You can reset the memory footprint to the size determined by benchmark with
the following button.

%% reset hashd-params            : [ Reset hashd parameters ]

You can re-run and cancel hashd benchmark with the following.

%% toggle bench-hashd            : Toggle hashd benchmark


___*Read on*___

%% jump intro.post-bench         : [ Next: Introduction to resctl-demo ]

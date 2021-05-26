pub const COMMON_DOC: &'static str = r#"
Resource Control Benchmark Common Concepts and Options
======================================================

When a system is under resource contention, various operating system
components and applications interact in complex ways. The interactions can't
easily be captured in synthetic per-component benchmarks making it difficult
to evaluate how the hardware and operating system would perform under such
conditions. resctl-bench solves the problem by exercising the whole system
with realistic workloads and analyzing system and workload behaviors.

Many of the concepts described here have detailed explanations in
resctl-demo. Give it a try.


rd-hashd
--------

rd-hashd is a simulated latency-sensitive request-servicing workload with
realistic system resource usage profile and contention responses. Its page
cache and heap access patterns follow normal distributions and load level is
regulated by both the target RPS and maximum response latency. The default
parameters are tuned so that resource-wise the behavior is rough an
approximation of a popular FB production workload.

While one workload can't possibly capture the many ways that systems are
used, rd-hashd's behaviors and requirements fall where many
human-interactive and machine-saturating workloads' would.

rd-hashd has its own sizing benchmark mode where it tries to figure out
parameters to saturate all of CPU, memory and IO. It finds the maximum RPS
that the CPUs can churn out and then figure out the maximum page cache and
heap footprint that the memory and IO can service. resctl-bench often uses
this benchmark mode, often with the cpu part faked, to evaluate IO devices.

For more details: `rd-hashd --help`


Memory Offloading and Profile
-----------------------------

Not all memory areas are equally hot. If the IO device is performant enough,
the tail-end of the access distribution can be offloaded without violating
latency requirements. Modern SSDs, even mainstream ones, can serve this role
in the memory hierarchy (https://en.wikipedia.org/wiki/Memory_hierarchy)
quite effectively by offloading page cache to filesystems and heap to swap.

This memory-offloading usage is critical not only because it makes expensive
memory to be used much more efficiently but also because this is what
happens when the system is under memory and IO contention. If the system
can't effectively handle memory offloading, the application
quality-of-service will be severely impacted under any resource pressure,
which lowers service reliability and forces further under-committing and
under-utilization of the systems.

resctl-bench uses how much of rd-hashd's memory footprint can be offloaded
to an IO device as the primary performance metric. It's often reported in
MOF (Memory Offloading Factor) whose definition is:

  SUPPORTABLE_MEMORY_FOOTPRINT / MEMORY_SIZE

For example, MOF of 1.2 means that the IO device can offload 20% of
available memory without violating service requirements. Note that both
bandwidth and latency contribute to MOF - bandwidth is meaningful only when
quick enough to meet the latency requirements.

The IO usage of memory offloading are influenced by the amount of available
memory. To ensure that bench results including MOFs are comparable across
different setups, resctl-bench uses memory balloon to constrain the amount
of available memory to a common value. This is called mem_profile (memory
profile) which is in gigabytes and always a power-of-two. The default
mem_profile is 16 and can be changed with the --mem-profile option.

The mem_profile of 16 tries to emulate a machine with 16G of memory. As not
all memory would be available for the workload, the net amount available for
rd-hashd is called mem_share. rd-hashd sizing benchmark is run with a bit
less memory to account for higher memory requirement for longer non-bench
runs. This amount is called mem_target.

resctl-bench needs to know how much memory is actually available to
implement mem_profile and automatically tries to estimate on demand. If the
available memory amount is already known (e.g. from the previous
invocation), --mem-avail can be used to skip this step. Some benchmarks
(storage and the wrapping benchmarks such as iocost-qos and iocost-tune) can
detect incorrect mem_avail and will retry automatically. Those benchmarks
may fail if the amount of available memory keeps fluctuating.


Nested IO Latency Distribution
------------------------------

resctl-bench reports IO completion latencies in a table which looks like the
following:

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

The "cum"ulative column shows the usual overall latency percentiles. For
example, in the above table, p99-cum (p99 row, cum column) is 10.5m
indicating that the 99th percentile of read completion latencies for the
whole benchmark was 10.5 milliseconds. While this already gives a fair bit
of insight, it can't distinguish, for example, devices which stall out most
requests in bursts from the usual spread-out long-tail high latency events
even though the former is a lot more disruptive.

resctl-bench calculates IO completion latency percentiles every second and
then determines the distribution of them over the whole run. In the above,
p50-99 - the p50 row, p99 column - is 1.5m, indicating that in one out of
100 1s periods, the median latency is gonna be as high as 1.5 millisecs.

Similarly, pNN-mean and pNN-stdev indicate the geometric average and
standard deviation of 1s NN'th percentile completion latencies over the
duration of the benchmark.


The Result File and Incremental Completion
------------------------------------------

"#;

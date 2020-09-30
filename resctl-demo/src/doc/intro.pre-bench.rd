## Copyright (c) Facebook, Inc. and its affiliates.
%% id intro.pre-bench: Welcome

*Welcome*\n
*=======*

We all want our workloads to utilize our machines as much as possible.
Toward that end, we spend considerable time and effort to tune the workloads
so that, ideally, under full load, they saturate the machines just enough so
that all available resources are utilized while leaving sufficient buffers
to keep the machines from falling apart when the maintenance cron jobs kicks
in at midnight.

In practice, this is nearly impossible to achieve reliably at scale forcing
us to under-commit many machines to have sufficient buffer for load surges.
Even with careful calibrations and dynamic machine allocations, it's
challenging to achieve high fleet-wide utilization while maintaining
quality-of-service and disaster readiness.

The idea behind resource control is to control distribution of resources
across workloads, so machine resources can be shared among different tasks
without tasks interfering with each other. This enables sizing resource
usage without worrying about maintenance workloads spikes and malfunctions
and pushing up machine utilization without sacrificing reliability,
responsiveness or disaster readiness.

The Facebook resource control demo, or resctl-demo, demonstrates resource
control in action: It uses workload simulators that mimic common resource
conflicts, and allows you to configure and test different strategies and
scenarios for reducing or eliminating resource contention. Because the demo
simulates conflicts typically found in large server deployments, the
strategies are directly applicable to real-world server fleets.

While resctl-demo concentrates on server scenarios, the resource control
strategies are generic and versatile, allowing direct translation to the
desktop and other personal device use cases. If we can protect the latency
profile of a web server under resource contention, we can protect your web
browser from stalling while the rest of the system is working on an
all-consuming build job.


___*Before you get started*___

The lower right pane is where all of your interaction with the demo takes
place. You can scroll in the pane by using the PageUp, PageDown, Home and
End keys, and move input focus with the Up and Down arrow keys. The input
focus can be moved to the log panes on the left and back with Left and Right
arrow keys. Enter activates the focused button. Left and Right arrow keys
move the focused slider.

Here are other keyboard shortcuts you may find useful:

* 'i': Index page. Jump to the index page, which has links to all pages.
  This page is "Welcome".

* 'g': Graph view. Press 'g' again or 'ESC' to close.

* 'l': Log view. Press 'l' again or 'ESC' to close.

* 'b': Back. Jump back to the last page.

* 'r': Reload. Reload the current page.

* 'q': Quit. Shut down everything and quit resctl-demo.

Some resctl-demo components require benchmarks for configuration that can
take longer than ten minutes to generate, so you should start them now. Keep
the system idle while the benchmarks are in progress. While you're waiting,
read the following information to get familiar with navigating the demo.

%HaveBench%___*Note*___: You already ran the benchmarks or are running the
official AWS image on the c5d.9xlarge machine type. Benchmark results are
already available and the following button won't do anything. If you want to
rerun the benchmarks, visit the iocost and hashd sub-pages.

___***WARNING***___: Benchmarks are run with resource control disabled and
there is a low probability of unrecoverable thrashing. If the system stalls
for over a minute, reset the machine and retry.

%% on bench-needed               : [ Start benchmarks ]

When resctl-demo is running benchmarks for the iocost IO controller and a
latency-sensitive workload simulator, called rd-hashd, in the top left
summary panel, the first line shows the current state and the latest
heartbeat timestamp. The state will first show "BenchIoCost" followed by
"BenchHashd" and finally "Running" when both benchmarks are complete.

There are a number of system and configuration requirements for the demo to
run. The second "config" row of the top left summary panel shows the number
of satisfied and missed requirements followed by resource control enable
status per resource type. For details on the requirements, follow the link
below. You can come back to this page by pressing 'b'.

%% jump intro.sysreqs            : [ System Requirements ]

The benchmarks try to calibrate resctl-demo so that the demo scenarios
behave as expected. However, resctl-demo is primarily verified on the
following two setups:

1. AMD Ryzen 7 3800X 8-Core 16-Threads CPU, 32G memory, Samsung 860 512G SSD

2. AWS c5d.9xlarge - 36 vCPUs, 72G memory, local 900G SSD

On setups that are significantly weaker than #1, the demo scenarios may not
behave as expected, especially on SSDs with high and inconsistent latency
profiles. When requests per second (RPS) suddenly dips or stays low, open
the graph view with 'g' and check out the IO utilization and read latency
graphs. Read latencies on some SSDs occasionally spike up to tens of
milliseconds even when the host isn't issuing overwhelming amount of IOs.
There is only so much the kernel can do for latency sensitive workloads when
a single IO takes tens of milliseconds.

The "Other logs" pane on the left shows what's going on. If the view is too
cramped, check out the fullscreen log view with the 'l' key. You can also
access the logs directly with `journalctl -u UNIT_NAME`. For the benchmarks,
the unit names are "rd-iocost-bench.service" and "rd-hashd-bench.service".

You can learn more about the iocost controller and hashd simulator, run the
benchmarks again, and tune and verify their results on the following pages.

%% jump intro.iocost             : [ Iocost Parameters and Benchmark ]
%% jump intro.hashd              : [ rd-hashd Workload Simulator ]

Otherwise, sit back, wait for the benchmarks to finish and the status change
to "Running", and then continue to the next page.

%% jump intro.post-bench         : [ Next: Introduction to resctl-demo ]

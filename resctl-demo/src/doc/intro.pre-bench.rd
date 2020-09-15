## Copyright (c) Facebook, Inc. and its affiliates.
%% id intro.pre-bench: Welcome
%% reset all-workloads
%% reset protections

*Welcome*\n
*=======*

The idea behind resource control is to distribute resources between
workloads, so machine resources can be shared among different tasks without
tasks interfering with each other.

resctl-demo demonstrates resource control in action: It uses workload
simulators that mimic common resource conflicts, and allows you to configure
and test different strategies and scenarios for reducing or eliminating
resource contention. Because the demo simulates conflicts typically found in
large server deployments, the strategies are directly applicable to
real-world server fleets.

While resctl-demo concentrates on server scenarios, the resource control
strategies are generic and versatile, allowing direct translation to the
desktop and other personal device use cases. If we can protect the latency
profile of a web server under resource contention, we can protect your web
browser from stalling while the rest of the system is thrashing on an
all-consuming build job.


___*Before you get started*___

This lower right pane is where all of your interaction with the demo takes
place. You can scroll it with the PageUp, PageDown, Home and End keys, and
move input focus with the Up and Down arrow keys. The input focus can be
moved to the log panes on the left and back with Left and Right arrow keys.
Enter activates the focused button. Left and Right arrow keys moves the
focused slider.

Here are other key bindings you may find useful.

* 'i': Jump to index page, with links to all pages. This page is "Welcome".

* 'g': Graph view. Press 'g' again or 'ESC' to close.

* 'l': Log view. Press 'l' again or 'ESC' to close.

* 'b': Back. Jump back to the last page.

* 'r': Reload. Reload the current page.

* 'q': Quit. Shut down everything and quit resctl-demo.

Some resctl-demo components require benchmarks for configuration, that can
take longer than ten minutes to generate, so you should start them now. Keep
the system idle while the benchmarks are in progress, and while you're
waiting, read on below to get familiar with navigating the demo.

%% on bench-needed               : [ Start benchmarks ]

***WARNING***: Benchmarks are run with resource control disabled and there
is a low probability of unrecoverable thrashing. If the system stalls for
over a minute, reset the machine and retry.

There are a number of system and configuration requirements for the demo to
run. The second "config" row of the top left summary panel shows the number
of satisfied and missed requirements followed by resource control enable
status per resource type. For details on the requirements, follow the link
below. You can come back to this page by pressing 'b'.

%% jump intro.sysreqs            : [ System Requirements ]

resctl-demo is currently running benchmarks for the iocost IO controller and
a test workload simulator, called rd-hashd. In the top left summary panel,
the first line shows the current state and the latest heartbeat timestamp.
The state will first show "BenchIoCost" followed by "BenchHashd" and finally
"Running" when both benchmarks are complete.

If you already ran the benchmarks or are running the official AWS image on
the c5d.9xlarge machine type, it should already be in the "Running" state.

The benchmarks try to calibrate resctl-demo so that the demo scenarios
behave as expected. However, resctl-demo is primarily verified on the
following two setups:

1. AMD Ryzen 7 3800X 8-Core 16-Threads CPU, 32G memory, Samsung 860 512G SSD

2. AWS c5d.9xlarge - 36 vCPUs, 72G memory, local 900G SSD

On setups which are significantly weaker than #1, the demo scenarios may not
behave as expected, especially on SSDs with wildy incosistent latency
profiles.

The "Other logs" pane on the left shows what's going on. If the view is too
cramped, check out the fullscreen log view with the 'l' key. You can also
access the logs directly with `journalctl -u UNIT_NAME`. For the benchmarks,
the unit names are "rd-iocost-bench.service" and "rd-hashd-bench.service".

You can learn more about the iocost controller and hashd simulator, run the
benchmarks again and tune and verify their results, on the following pages.

%% jump intro.iocost             : [ Iocost Parameters and Benchmark ]
%% jump intro.hashd              : [ rd-hashd Workload Simulator ]

Otherwise, sit back, wait for the benchmarks to finish and the status change
to "Running", and then continue to the next page.

%% jump intro.post-bench         : [ Next: Introduction to resctl-demo ]

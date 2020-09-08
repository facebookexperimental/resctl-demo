## Copyright (c) Facebook, Inc. and its affiliates.
%% id intro.pre-bench: Welcome
%% reset all-workloads
%% reset protections

*Welcome*\n
*=======*

The idea behind resource control is to distribute resources between workloads,
so machine resources can be shared among different tasks without tasks
interfering with each other.

resctl-demo demonstrates resource control in action: It uses workload simulators
that mimic common resource conflicts, and allows you to configure and test
different strategies and scenarios for reducing or eliminating resource
contention. Because the demo simulates conflicts typically found in large server
deployments, the strategies are directly applicable to real-world server fleets.

___*Before you get started*___

There are a number of system and configuration requirements for the demo to run:
Follow the following link to make sure your system is configured to run
resctl-demo.

%% jump intro.sysreqs            : [ System Requirements ]

Some resctl-demo components require benchmarks for configuration, that can take
longer than ten minutes to generate, so you should start them now. Keep the
system idle while the benchmarks are in progress, and while you're waiting, read
on below to get familiar with navigating the demo.

%% on bench-needed               : [ Start benchmarks ]

This lower right pane is where all of your interaction with the demo takes
place. You can scroll it with the PageUp, PageDown, Home and End keys, and move
input focus with the Up and Down arrow keys. The input focus can be moved to the
log panes on the left and back with Left and Right arrow keys.

We'll get into more detail later, but here are a couple key bindings you may
find useful in the meantime.

* 'i': Jump to index page, with links to all pages. This page is "Welcome".

* 'g': Graph view. Press 'g' again or 'ESC' to close.

* 'l': Log view. Press 'l' again or 'ESC' to close.

resctl-demo is currently running benchmarks for the iocost IO controller and a
test workload simulator, called hashd. In the top left summary panel, the first
line shows the current state and the latest heartbeat timestamp. The state will
first show [BenchIoCost] followed by [BenchHashd] and finally [Running] when
both benchmarks are complete. If you already ran the benchmarks or are running
the official resctl-demo image on the c5d.9xlarge machine type, it should
already be in the Running state.

The "Other logs" pane on the left shows what's going on. If the view is too
cramped, check out the fullscreen log view with the 'l' key. You can also access
the logs directly with 'journalctl -u UNIT_NAME'. For the benchmarks, the unit
names are "rd-iocost-bench.service" and "rd-hashd-bench.service".

You can learn more about the iocost controller and hashd simulator, run the
benchmarks again and tune and verify their results, on the following pages.

%% jump intro.iocost             : [ Iocost Parameters and Benchmark ]
%% jump intro.hashd              : [ rd-hashd Workload Simulator ]

Otherwise, sit back, wait for the benchmarks to finish, and then continue to the
next page.

%% jump intro.post-bench         : [ Next: Introduction to resctl-demo ]
%% jump index                    : [ Exit: Index ]

## Copyright (c) Facebook, Inc. and its affiliates.
%% id intro.pre-bench: The Benchmarks
%% reset all-workloads
%% reset protections

*Welcome*\n
*=======*

The idea behind resource control is to distribute resources between
workloads so that machines can be shared among different tasks without them
interfering with each other. resctl-demo demonstrates resource control in
action using a number of test workloads.

Some components used by resctl-demo require benchmarks for configuration. As
they can take longer than ten minutes, let's start them right away. Please
keep the system otherwise idle while the benchmarks are in progress and read
on.

%% on bench-needed               : [ Start benchmarks ]

This lower right pane is where all of your interaction with this demo will
take place. You can scroll it with the PageUp, PageDown, Home and End keys
and move input focus with the Up and Down arrow keys. The input focus can be
moved to the log panes on the left and back with Left and Right arrow keys.

We'll get to proper introduction once bench results are ready but here are a
couple key bindings you may find useful in the meantime.

* 'i': Jump to index page which links to all pages. This page is "The
  Benchmarks".

* 'g': Graph view. Press 'g' again or 'ESC' to close.

* 'l': Log view. Press 'l' again or 'ESC' to close.

resctl-demo is currently running benchmarks for two components - iocost and
hashd. The "Other logs" pane on the left should be showing what's going on.
If the view is too cramped, check out the fullscreen log view with 'l' key.
You can also access the logs directly with 'journalctl -u UNIT_NAME'.

If you wanna learn more about what they are and why they need benchmarking,
and tune and verify the bench results, please visit the following pages.

%% jump intro.iocost             : [ io.cost Controller ]
%% jump intro.hashd              : [ rd-hashd Workload Simulator ]

Otherwise, please sit back and wait the benchmarks to finish and then
continue to the next page.

%% jump intro.post-bench         : [ Next: Introduction ]
%%
%% jump index                    : [ Exit: Index ]

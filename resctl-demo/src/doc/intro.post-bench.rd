## Copyright (c) Facebook, Inc. and its affiliates.
%% id intro.post-bench : Welcome to resouce control demo
%% reset all-workloads
%% reset protections

*Introduction*\n
*============*

%WarnBench%

The idea behind resource control is to distribute resources between
workloads so that machines can be shared among different tasks without
them interfering with each other.

## insert examples: use web browser while compiling, run a webserver
## safely while the machine does rpm upgrades etc.

for this purpose, let's introduce a test workload we're going to use
for this demonstration, called hashd. hashd sets up test files from
which it serves data requests and measures the end-to-end latency of
each request. it tries to serve as many requests as it can without
sacrificing response time. this is similar to how webservers or
workloads like memcache operate in load-balanced compute pools. but
because it's highly sensitive to latencies, it can also stand in for
other applications, such as browsers or similar interactive desktop or
mobile applications: their requests per seconds might be fewer, but
the user cares very much about the latency behind each one.

hashd is sensitive to the steady availability of cpu, memory and io,
and so it'll be an honest indicator of how well resource isolation is
working on all fronts on this host.

let's fire up hashd to get rolling

%% on hashd : [ Start hashd ]

watch the panel to your left to see the rps ramping up. you can check
the logs for warnings on errors as well. (explain more of the panels
as they become relevant.)

okay, now that our main workload is running, let's see how it responds
to competition. for this purpose, we're going to launch a compile job.
because it's not interactive, and thereby not bound by user input, it
will eat up as many cpu cycles, as much io bandwidth, and as much
memory for caches that it can get its hands on. it's doing useful
work, but it's the perfect antagonist to our interactive hashd.

%% on sideload test-build build-linux-4x : [ Start a linux build sideload ]

ok, see the graph for how hashd rps take a shit. that's the compile
job taking away its resources. not good.

%% reset all : [ Enable Resource Control ]

watch the rps stabilize.

see how the compile job still makes forward progress.

these two workloads are now sharing the machine safely, something that
wouldn't have been possible before.

## continue to more severe disturbances, introduce freezing &
## oomkilling

## continue to disable all sideloads and show how the workload's
## memory footprint grows without the rps actually going up. introduce
## senpai as a means to provide an accurate measure of memory headroom

## continue to the advanced page that breaks down the components of
## resource control, allow the user to disable some aspects and
## explain what fails and how, mention prio inversions etc.

%% jump index                    : [ Exit: Index ]

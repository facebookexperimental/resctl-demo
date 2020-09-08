## Copyright (c) Facebook, Inc. and its affiliates.
%% id intro.post-bench : Introduction to resouce control demo
%% reset all-workloads
%% reset protections

*Introduction*\n
*============*

%WarnBench%

The idea behind resource control is to distribute resources between workloads so
that machines can be shared among different tasks without them interfering with
each other.

The sharing workloads can be the web browser and kernel compilation job on your
laptop, or a web server and maintenance workloads such as package upgrades and
cron jobs. Maybe we want to transcode videos to utilize the unused capacities of
the web server.

For this demo, we're going to use the test workload called hashd. hashd sets up
test files from which it serves data requests, and measures the end-to-end
latency of each request. It tries to serve as many requests as it can without
sacrificing response time, similar to how web servers or workloads like memcache
operate in load-balanced compute pools. Because it's highly sensitive to
latencies, it can also stand in for other applications, such as browsers or
similar interactive desktop or mobile applications: While their
requests-per-second might be fewer, low latency and fast response time are top
priorities for users.

hashd's sensitivity to the steady availability of CPU, memory, and IO, makes it
an honest indicator of how well resource isolation is working on all fronts on
the host.

Let's fire up hashd to get rolling:

%% on hashd                      : [ Start hashd ]

Watch the panel to your left to see the RPS ramping up. You can check the logs
for warnings and errors as well.

OK, now that our main workload's running, let's see how it responds to
competition. For this purpose, we're going to turn off resource control and
launch a compile job. Because it's not interactive, and therefore not bound by
user input, it will eat up as many CPU cycles, as much IO bandwidth, and as much
memory for caches as it can get its hands on. It's doing useful work, but it's
the perfect antagonist to our interactive hashd.

%% (                             : [ Disable resource control and start linux build job ]
%% off cpu-resctl
%% off mem-resctl
%% off io-resctl
%% on sysload build-linux-2x build-linux-2x
%% )

See the graph for the steep drop in RPS for hashd: That's the compile job taking
away its resources: Not good.

Now let's stop the build job and restore resource control:

%% (                             : [ Stop linux build job and restore resource control ]
%% off sysload build-linux-2x
%% reset resctl
%% )

Once RPS climbs back up and stabilizes, let's start the same build job with
resource control enabled and under the supervision of the sideloader:

%% (                             : [ Start linux build job as a sideload ]
%% on sideload build-linux-2x build-linux-2x
%% )

Watch the stable RPS while the compile job still makes forward progress: These
two workloads are now sharing the machine safely, something that wasn't possible
before.

Continue reading to learn more about the various components which make this
possible.

%% jump comp.cgroup              : [ Next: Cgroup and Resource Protection ]
%% jump index                    : [ Exit: Index ]

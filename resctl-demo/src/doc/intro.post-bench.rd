## Copyright (c) Facebook, Inc. and its affiliates.
%% id intro.post-bench : Introduction to resouce control demo
%% reset all-workloads
%% reset protections

*Introduction*\n
*============*

%WarnBench%

The idea behind resource control is to distribute resources between
workloads so that machines can be shared among different tasks without them
interfering with each other.

The sharing workloads can be the web browser and kernel compilation job on
your laptop, or a web server and maintenance workloads such as package
upgrades and cron jobs. Maybe we want to transcode videos to utilize the
unused capacities of the web server.

For this demo, we're going to use the test workload called rd-hashd.
rd-hashd sets up test files from which it serves data requests, and measures
the end-to-end latency of each request. It tries to serve as many requests
as it can without sacrificing response time, similar to how web servers or
workloads like memcache operate in load-balanced compute pools. Because it's
highly sensitive to latencies, it can also stand in for other applications,
such as browsers or similar interactive desktop or mobile applications:
While their requests-per-second might be fewer, low latency and fast
response time are top priorities for users.

rd-hashd's sensitivity to the steady availability of CPU, memory, and IO,
makes it an honest indicator of how well resource isolation is working on
all fronts on the host.

Let's fire up rd-hashd to get rolling:

%% (                             : [ Start rd-hashd at 60% load ]
%% knob hashd-load 0.6
%% on hashd
%% )

Watch the panel to your left to see the RPS ramping up. You can check the logs
for warnings and errors as well.

OK, now that our main workload's running, let's see how it responds to
competition. For this purpose, we're going to turn off resource control and
launch a compile job and a memory hog. The former will eat up as many CPU
cycles as it can get its hands on along with some memory and IO bandwidth.
The latter will keep gobbling up memory causing memory shortage and
subsequent IOs once memory is filled up. The combination is a potent
antagonist to our interactive rd-hashd.

%% (                             : [ Disable resource control and start the competitions ]
%% off cpu-resctl
%% off mem-resctl
%% off io-resctl
%% on sysload compile-job build-linux-2x
%% on sysload memory-hog memory-growth-50pct
%% )

See the graph for the steep drop in RPS for hashd: That's the competitions
taking away its resources: Not good.

Once workload's memory pressure in the top right panel starts spiking, you
might not have a lot of time before the whole system starts stalling
severely. Let's stop them.

%% (                             : [ Stop the compile job and memory hog ]
%% reset secondaries
%% )

Once RPS climbs back up and stabilizes, start the same competitions but with
resource control enabled and the compile job under the supervision of the
sideloader:

%% (                             : [ Start the competitions under full resource control ]
%% reset resctl
%% on sideload compile-job build-linux-2x
%% on sysload memory-hog memory-growth-50pct
%% )

Watch the stable RPS. rd-hashd is now fully protected against the
competitions. The compile job and memory hog are throttled. The compile job
doesn't seem to be making much progress. This is because sideloads
(workloads under the siderloader supersivision) are configured to have lower
priority than sysloads (workloads under systems.slice). Don't worry about
the distinction between sideloads and sysloads for now. We'll revisit them
later.

Let's stop the memory hog and see what happens.

%% (                             : [ Stop the memory hog ]
%% off sysload memory-hog
%% )

rd-hashd is still doing fine and the compile job is now making reasonable
forward progress: These two workloads are now sharing the machine safely and
productively, something that wasn't possible before.

Continue reading to learn more about the various components which make this
possible.

%% jump comp.cgroup              : [ Next: Cgroup and Resource Protection ]

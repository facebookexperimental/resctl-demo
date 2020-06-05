## Copyright (c) Facebook, Inc. and its affiliates.
%% id senpai.intro: The Problem of Sizing Memory
%% reset secondaries
%% reset protections
%% knob hashd-load 1.0
%% on hashd

*The Problem of Memory Sizing*\n
*============================*

We want to know how much memory a given workload needs. If we allocate too
little, the workload will thrash and won't be productive. If we give too
much, we're just wasting memory which can be used for something more useful.
As we try to push up utilization with stacking and sideloading, this becomes
even more important. We gotta know how much memory is available for other
workloads to stack and sideload.

With PSI memory pressure, we can tell how bad memory shortage is, so that's
one side of the scale - we can tell when a workload needs more. Can we tell
when a workload has more than enough memory tho?

rd-hashd should be running at full load already. Wait until its memory usage
doesn't climb anymore. It should be filling most of the machine. Let's
reduce the load to 25%.

%% knob hashd-load 0.25          : [ Reduce rd-hashd load level to 25% ]

Notice how RPS falls but memory usage stays the same. Memory and IO
pressures should be really low or zero. There are some writes for the logs
but not much reads. It sure isn't contending for memory and looks like it
might have more memory than it needs, but we can't really tell whether or
how much.

This is because memory management is fundamentally lazy. Memory is accessed
a lot. Bandwidth can be tens of gigabytes. That's a lot of pages. If we
tried to track each page use, we'd be using a significant portion of the
system just for that, which nobody wants. We want machines to do actual
work. So, instead, the kernel tracks as little as possible as lazy as
possible.

When memory starts to get short, the kernel starts scanning the pages
learning which pages are being accessed. When all pages are in use and some
need to be reclaimed, kernel picks what it thinks are cold pages and
reclaims them. The choices aren't gonna be perfect and some pages might need
to be brought back right away, which becomes another datapoint for the
hotness of the page. As this process continues, the kernel's understanding
of which pages are hot and which aren't becomes more accurate.

Reclaim activities are what informs memory management of the access
patterns. Without on-going reclaim, the kernel continues to lose its
understanding of memory usage ultimately to the point where it can't tell
whether any one page is hotter than any other page. In this state, *nothing*
in the system knows which pages are being actively used and which aren't.

So, it's no surprise that we can't find out how much more memory rd-hashd
has than it actually needs. rd-hashd was using all that memory and then its
usage shrunk but the now cold pages didn't get destroyed as they may be used
again in the future. As there's no memory shortage, reclaim doesn't happen
anymore and time passes the source information of memory hotness disappears.
The only thing knowable is that there is enough to avoid triggering reclaim.

There's nothing special about rd-hashd's behavior. In most cases, memory
will be filled up with cold pages from files accessed a couple hours ago,
memory areas which were used during init but not discarded, browser tabs
that you left open since yesterday. We don't want memory to go unused or do
extra work managing them when not needed, so this is actually what we want
in many cases.

However, we really want to know how much memory is actually *needed* by a
workload as that directly connects to how efficiently we can utilize our
fleet of machines and are willing to pay some overhead, hopefully a small
bit, for that. How can we do it?


___*Read on*___

Now that we understand what the problem is, let's read on to find out how it
can be solved.

%% jump senpai.senpai            : [ Next: Senpai ]
%% jump side.exp                 : [ Prev: Experiment with Sideloading ]
%% jump index                    : [ Exit: Index ]

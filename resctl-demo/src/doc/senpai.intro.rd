## Copyright (c) Facebook, Inc. and its affiliates.
%% id senpai.intro: The Problem of Sizing Memory
%% reset secondaries
%% reset protections
%% knob hashd-load 1.0
%% on hashd

*The Problem of Memory Sizing*\n
*============================*

We want to know how much memory a given workload needs. Allocating too
little makes the workload thrash unproductively. Too much, and we're just
wasting memory that could be used for something more useful. Accurate memory
sizing becomes even more important as we try to push utilization up with
stacking and sideloading. We need to know how much memory's available for
other workloads to stack and sideload.

With PSI memory pressure, we can see how bad a memory shortage is, so that's
one side of the scale - we can tell when a workload needs more. But can we
tell when a workload has more than enough memory?

rd-hashd should be running at full load already. Wait until its memory usage
doesn't climb anymore. It should be filling most of the machine. Let's
reduce the load to 25%:

%% knob hashd-load 0.25          : [ Reduce rd-hashd load level to 25% ]

Notice how RPS falls but memory usage stays the same. Memory and IO pressure
should be really low or zero. There are some writes for the logs but not
many reads. rd-hashd sure isn't contending for memory, and it looks like it
might have more memory than it needs, but we can't really tell whether
that's true, or by how much.

This is because memory management is fundamentally lazy. Memory is accessed
a lot, and bandwidth can be tens of gigabytes - that's a lot of pages. If we
tried to track each page use, we'd use a significant portion of the system
just for that, which nobody wants - we want machines to do actual work. So
instead, the kernel tracks page use as little as possible, and as lazily as
possible.

When memory starts getting scarce, the kernel starts scanning the pages to
learn which pages are being accessed. When all pages are in use and some
need to be reclaimed, the kernel picks what it thinks are cold pages and
reclaims them. The choices aren't going be perfect, and some pages might
need to be brought back right away, which becomes another datapoint for the
hotness of the page. As this process continues, the kernel's understanding
of which pages are hot and which aren't becomes more accurate.

These reclaim activities inform memory management of the access patterns.
Without ongoing reclaim, the kernel continues to lose its understanding of
memory usage, ultimately to the point where it can't tell whether any one
page is hotter than any other page. In this state, *nothing* in the system
knows which pages are actively used and which aren't.

So, it's no surprise we can't determine how much more memory rd-hashd has
than it actually needs. rd-hashd was using all that memory, and then its
usage shrunk, but the now cold pages didn't get destroyed since they might
be used again in the future. Since there's no memory shortage, reclaim
stops, and as time passes, the source of memory hotness information
disappears. The only thing we know for certain is that there's enough to
avoid triggering reclaim.

There's nothing special about rd-hashd's behavior. In most cases, memory is
filled up with cold pages from files accessed hours earlier, memory areas
used during init but that weren't discarded, or browser tabs you left open
since yesterday. We don't want memory to go unused, or do extra management
work when not needed, so this behavior is actually what we want in many
cases.

But for memory sizing, we really want to know how much memory a workload
actually *needs*, even at the cost of a small bit of overhead, since that
information directly impacts how efficiently we can utilize our fleet. So,
when normal reclaim ceases, how can we get this memory management
information, and at almost no cost?


___*Read on*___

Now that we understand the problem of memory sizing, read on for the
solution.

%% jump senpai.senpai            : [ Next: Senpai ]

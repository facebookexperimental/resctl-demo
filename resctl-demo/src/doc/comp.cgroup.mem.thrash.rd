## Copyright (c) Facebook, Inc. and its affiliates.
%% id comp.cgroup.mem.thrash: The Anatomy of Thrashing
%% reset all-workloads
%% reset protections
%% knob hashd-load 1.0
%% on hashd
$$ reset hashd-params

*The Anatomy of Thrashing*\n
*========================*

When a system enters a thrashing state where it spends most of the time
waiting on IOs while not making much, if any, progress, what's going on? Why
is memory shortage tied so closely together with IO?

When a program loads and allocates memory, it's respectively mapping file
backed memory pages and allocating anonymous pages. The memory pages,
usually 4kb in size, aren't actual physical memory pages. They're only
promised to be available with the right content when the program later tries
to use them - thus the terms virtual memory and on-demand paging.

As the pages are given out, eventually, the system will run out of unused
pages. When, for example, the program jumps to a new code page, the kernel
has to make space somehow. The ideal solution would be finding a page which
will never be used again and then recycle it for the new page. To
generalize, it'd be best to recycle pages which are the least likely to be
reused in the future. The kernel can't reliably predict the future but it
can make educated guesses based on past history. The process of picking the
pages to recycle and recycling them is called page reclaim.

Let's say you have a tiny workload and have ten pages (whopping 40k) to run
it. Right now, it's happy with 9 pages but slowly growing. When it reaches
10 pages, we give out the last remaining one, and start gathering
information on which page is being used. When it wants to grow to 11 pages,
we pick one page, kick it out (write out to disk if needed) and recycle that
page for the 11th. As it grows further, we keep doing that.

Let's expand our thought experiment with a few more assumptions. Five of the
ten pages are hot - the program accesses them frequently. The other pages
are visited one per second one-by-one. If the current program size is 13
pages, 5 of the pages can't be kicked out because they're hot and will be
brought back in right away, so 8 program pages have to be served by 5 memory
pages. Every second, three pages have to be kicked out and brought back from
the storage device.

As long as the storage device can serve three pages per second quickly
enough, our program can run fine. However, if the program size keeps
growing, the demand on the storage devices will grow together and at some
point it'll start falling behind. The program won't be able to make its
round through its pages within the second and will start falling behind.

Let's consider a little bit more realistic scenario. Let's say both the hot
and cool parts of the program grow. Hot pages are cyclically accessed 1000
times a second and cold pages once. Hot:cold starts at 5:5 and both grow at
the same rate. Because one hot page can cause upto 1000 page faults while a
cold one can cause only upto 1, we have to keep the hot ones on memory as
much as possible.

When it's 6:6, 6 hot ones in memory, 6 cold ones will share 4 memory pages,
minimum 2 page faults per second. When it's 9:9, 9 hot ones in memory, 9
cold ones will share 1 memory page - 8 faults. When 10:10, the 10 hot pages
must mostly stay in memory and the cold ones have to cycle through inbetween
hot page accesses - 20 faults. When 11:11, all hell breaks loose. Each round
through the hot pages will require at least one page fault. The absolute
minimum page faults per second is now above 1000.


___*Thrashing in action*___

rd-hashd's memory access pattern follows a normal distribution. By default,
the standard deviation is one-fourth of the mean - the access frequencies
from the hottest to the coldest quarters are approximately 68%, 27%, 4% and
0.2%. There is a pretty hot core and a sizable cold tail.

The benchmark configured rd-hashd so that the machine is nearly saturated on
both memory and IO axes at the full load. Memory is serving the hot part and
IO the cold. If you compare benchmarked memory footprint (%HashdMemSize%) to
the available memory minus a few gigs for the kernel and rest of the system,
the delta is what's being served from IO device.

We can approximate the first scenario from the previous section - cold
memory footprint expanding - by scaling the memory footprint of hashd with
default parameters. If we expand the memory footprint, we'd be pushing more
and more cold memory over to the IO.

rd-hashd should already be running at the full load. Try adjusting memory
footprint with the following slider and watch how RPS and IO usage change.

%% knob   hashd-mem              : memory footprint :

Notice how IO usage goes away when you slide it lower and gradually increase
as you push it up. Eventually, the IO device won't be able to serve fast
enough and RPS will start dropping as you'd expect from the cold(er) memory
expansion.

Now, let's try the second scenario - the cliff behavior when hot working set
expands beyond memory capcity. To approximate the behavior, we'll make hashd
memory access pattern uniform so that all memory is accessed uniformly.

%% (                             : [ Set uniform access pattern and reduce memory footprint ]
%% knob hashd-addr-stdev 1.0
%% knob hashd-mem 0.01
%% )

Once hashd settles, increase the memory footprint using the slider above. Up
until total memory minus a couple gigabytes, it'll rise without any
resistance and there won't be much IO activities except for log writes. Once
you're close to the total amount of available memory, give it some time to
stabilize and keep inching it up slowly while watching IO usage.

Depending on the IO device, the workload may go from running fine to barely
running at all in a single click of the slider or it may be able to hold out
quite a bit. But it will transition from not much IO to a lot of IOs with
RPS lossage a lot quicker than in the previous experiment.

You can reset rd-hashd parameters with the following button.

%% (                             : [ Reset rd-hashd parameters ]
%% reset hashd-params
%% knob hashd-load 1.0
%% )


___*Read on*___

We examined how memory reclaim works and why thrashing happens and
reproduced the behaviors with rd-hashd. Now that we understand how memory
management and IO are intertwined, let's take a look at IO control.

%% jump comp.cgroup.io           : [ Next: IO Control ]
%% jump comp.cgroup.mem          : [ Prev: Memory Control ]
%% jump comp.cgroup              : [ Up: Cgroup and Resource Protection ]
%% jump index                    : [ Exit: Index ]

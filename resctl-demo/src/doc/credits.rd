## Copyright (c) Facebook, Inc. and its affiliates.
%% id credits: Credits
%% reset prep

*Credits*\n
*=======*

Copyright (c) Facebook, Inc. and its affiliates.\n
Apache License, Version 2.0\n
October, 2020

The architectural and interface design of cgroup2, and all the big picture
directions on the applied resource control strategies are born of the
collaboration between *Tejun Heo* and *Johannes Weiner* over the past eight
years.

*Johannes Weiner* developed and led everything memory management related,
including most of the cgroup2 memory controller and anonymous memory
balancing. Johannes also developed PSI, guided its application in OOMD, and
created Senpai on top.

*Josef Bacik* developed `io.latency` - the first working comprehensive IO
controller. While this demo doesn't use `io.latency`, IO control works only
because of the many improvements that Josef made across the kernel in block
layer, filesystem, memory management, and read-ahead.

*Andy Newell* created the theoretical base for the `io.cost` controller,
devised the multiple node weight update algorithm, which improved work
conservation and control quality.

*Tejun Heo* worked on the cgroup core, implemented `io.cost` based on Andy's
work and the experience gained from `io.latency`. Tejun implemented the
prototype OOMD and sideloader.

On btrfs, *Josef Bacik* and *Chris Mason* debugged and fixed many priority
inversion issues. *Omar Sandoval* added swapfile support. *Dennis Zhou*
implemented lazy async discard greatly expanding the range of acceptable
SSDs.

*Roman Gushchin* implemented the cgroup2 freezer and made numerous memory
controller and other cgroup contributions with focus on memory efficiency.
Roman also led the early protection scenario experiments.

*Chris Down* implemented `memory.high` artificial delay and provided support
for many resource control initiatives.

*Rik van Riel* and *Song Liu* worked on the CPU controller.

*Dan Schatzberg* was one of the earliest adoptors of resource control in
Facebook and is leading the team implementing resource control in
production. Dan also made numerous technical, conceptual and directional
contributions to OOMD, Senpai and other resource control projects.

*Daniel Xu* took the primitive prototype and built the fully-fledged
production-worthy OOMD.

*Aravind Anbudurai* and *Davide Cavalca* played a key role in early
deployment of cgroup2 in Facebook.

resctl-demo is written by *Tejun Heo* with the contributions from *Johannes
Weiner*, *Thomas Connally*, and *Penni Johnson*.

The AWS and installable images are built by *Christopher Obbard* of
Collabora. Thanks to *Guy Lunardi* and *Angelica Ramos* for support from
Collabora.

None of these would have been possible without the Linux kernel and the many
open source projects we depend and build upon everyday. Our deep gratitude
goes out to the broader Linux community for their feedback, bug reports,
code review, and discussions.


%% jump index                    : [ Go back to index ]

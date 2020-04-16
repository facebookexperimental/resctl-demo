## Copyright (c) Facebook, Inc. and its affiliates.
%% id doc-format                 : Doc markup format

*resctl-demo doc markup format*\n
*=============================*

Lines which are "##" or start with "## " are considered comments and ignored.
Lines which don't start with one of the special markers - "##", "%%" or "$$" -
are regular paragraphs and follow the following rules.

* Blank or whitespace-only lines separate paragraphs. The number of spaces or
  whitespaces don't make a difference.

* Inside a paragraph, * s and _ s can be used to apply different styles. *BOLD*,
  **ALERT**, ***REVERSED_ALERT***, ___UNDERLINED___. Underlines can be combined
  with the other styles - **___UNDERLINED_ALERT___**.

* Whitespaces at the end of a paragraph line are trimmed. Whitespaces at the
  beginning are kept.

* Plain, itemized and numbered indentations are supported.

    This should be indented by four spaces. Lorem ipsum dolor sit amet,
    consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et
    dolore magna aliqua.

      This should be indented by six spaces. Lorem ipsum dolor sit amet,
      consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et
      dolore magna aliqua.

  * This should be indented by four spaces. Lorem ipsum dolor sit amet,
    consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et
    dolore magna aliqua.

    * This should be indented by six spaces. Lorem ipsum dolor sit amet,
      consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et
      dolore magna aliqua.

  1. This should be indented by five spaces. Lorem ipsum dolor sit amet,
     consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et
     dolore magna aliqua.

     2. This should be indented by eight spaces. Lorem ipsum dolor sit amet,
        consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore
        et dolore magna aliqua.

Lines which are "%%" are empty paragraphs and can be used to create vertical
spacing.

Lines which start with "%% " are commands and have the following syntax.

  ## CMD TARGET [ARG] [: PROMPT]

If prompt is not specified, the command is run when the page becomes active. If
prompt is specified, the prompt and the appropraite user-interactable UI element
is embedded in the document os that the user can trigger the command.

Lines which start with "$$ " are epilog commands and have the following syntax.

  $$ CMD TARGET [ARG]

The specified command is executed when jumping out of the page.

The followings are all available commands.

%% on     bench-iocost           : [ Start iocost benchmark ]
%% off    bench-iocost           : [ Stop iocost benchmark ]
%% toggle bench-iocost           : Toggle iocost benchmark
%%
%% on     bench-hashd            : [ Start hashd benchmark ]
%% off    bench-hashd            : [ Stop hashd benchmark ]
%% toggle bench-hashd            : Toggle hashd benchmark
%%
%% on     bench-hashd-loop       : [ Start hashd benchmark loop ]
%% off    bench-hashd-loop       : [ Stop hashd benchmark loop ]
%% toggle bench-hashd-loop       : Toggle hashd benchmark loop
%%
%% on     hashd                  : [ Start hashd ]
%% off    hashd                  : [ Stop hashd ]
%% toggle hashd                  : Toggle hashd
%%
%% on     hashd-B                : [ Start the second instance of hashd ]
%% off    hashd-B                : [ Stop the second instance of hashd ]
%% toggle hashd-B                : Toggle the second instance of hashd

A sideload is identified with the tag, "test-build" here. The following job ID
points to an entry in sideload-defs.json and determines the specific workload.

%% on sideload test-build build-linux-4x          : [ Start a linux build sideload ]
%% off sideload test-build                        : [ Stop a linux build sideload ]
%% toggle sideload test-build build-linux-4x      : Toggle a linux build sideload
%%
%% on sideload test-mem memory-growth-25pct       : [ Start a 25% memory growth sideload ]
%% off sideload test-mem                          : [ Stop a 25% memory growth sideload ]
%% toggle sideload test-mem memory-growth-25pct   : Toggle a 25% memory growth sideload
%%
%% on sideload test-io read-bomb                  : [ Start a read bomb sideload ]
%% off sideload test-io                           : [ Stop a read bomb sideload ]
%% toggle sideload test-io read-bomb              : Toggle a read bomb sideload

A sysload is a sideload which is run under system.slice without the supervision
of sideloader and can be used to illustrate oomd workload protection or the need
for sideloader.

%% on sysload test-build build-linux-4x          : [ Start a linux build sysload ]
%% off sysload test-build                        : [ Stop a linux build sysload ]
%% toggle sysload test-build build-linux-4x      : Toggle a linux build sysload
%%
%% on sysload test-mem memory-growth-25pct       : [ Start a 25% memory growth sysload ]
%% off sysload test-mem                          : [ Stop a 25% memory growth sysload ]
%% toggle sysload test-mem memory-growth-25pct   : Toggle a 25% memory growth sysload
%%
%% on sysload test-io read-bomb                  : [ Start a read bomb sysload ]
%% off sysload test-io                           : [ Stop a read bomb sysload ]
%% toggle sysload test-io read-bomb              : Toggle a read bomb sysload
%%
%% on     cpu-resctl             : [ Turn on CPU resource protection ]
%% off    cpu-resctl             : [ Turn off CPU resource protection ]
%% toggle cpu-resctl             : Toggle CPU resource protection
%%
%% on     mem-resctl             : [ Turn on memory resource protection ]
%% off    mem-resctl             : [ Turn off memory resource protection ]
%% toggle mem-resctl             : Toggle memory resource protection
%%
%% on     io-resctl              : [ Turn on IO resource protection ]
%% off    io-resctl              : [ Turn off IO resource protection ]
%% toggle io-resctl              : Toggle IO resource protection
%%
%% on     oomd                   : [ Turn on OOMD ]
%% off    oomd                   : [ Turn off OOMD ]
%% toggle oomd                   : Toggle OOMD
%%
%% on     oomd-work-mem-pressure : [ Turn on memory pressure protection in workload.slice ]
%% off    oomd-work-mem-pressure : [ Turn off memory pressure protection in workload.slice ]
%% toggle oomd-work-mem-pressure : Toggle memory pressure protection in workload.slice
%%
%% on     oomd-work-senpai       : [ Turn on senpai in workload.slice ]
%% off    oomd-work-senpai       : [ Turn off senpai in workload.slice ]
%% toggle oomd-work-senpai       : Toggle senpai in workload.slice
%%
%% on     oomd-sys-mem-pressure  : [ Turn on memory pressure protection in system.slice ]
%% off    oomd-sys-mem-pressure  : [ Turn off memory pressure protection in system.slice ]
%% toggle oomd-sys-mem-pressure  : Toggle memory pressure protection in system.slice
%%
%% on     oomd-sys-senpai        : [ Turn on senpai in system.slice ]
%% off    oomd-sys-senpai        : [ Turn off senpai in system.slice ]
%% toggle oomd-sys-senpai        : Toggle senpai in system.slice

A knob configures a value between 0.0 and 1.0. A knob command should either have
a value argument or prompt.

%% knob   hashd-load 0.6
%% knob   hashd-load             : Main workload load level                 :
%% knob   hashd-mem              : Main workload memory footprint           :
%% knob   hashd-file             : Main workload pagecache proportion       :
%% knob   hashd-file-max         : Main workload max pagecache proportion   :
%% knob   hashd-log-bps          : Main workload log write bandwidth        :
%% knob   hashd-weight           : Main workload weight                     :
%% knob   hashd-B-load           : Second workload load level               :
%% knob   hashd-B-mem            : Second workload memory footprint         :
%% knob   hashd-B-file           : Second workload pagecache proportion     :
%% knob   hashd-B-file-max       : Second workload max pagecache proportion :
%% knob   hashd-B-log-bps        : Second workload log write bandwidth      :
%% knob   hashd-B-weight         : Second workload weight                   :
%%
%% knob   sys-cpu-ratio          : system CPU weight compared to workload   :
%% knob   sys-io-ratio           : system IO weight compared to workload    :
%% knob   mem-margin             : Memory for the rest of the system        :
%% knob   balloon                : Memory balloon size                      :

Reset commands are shortcuts to restore to default configurations.

%% reset  benches                : [ Stop hashd and iocost benchmarks ]
%% reset  hashds                 : [ Stop hashd instances ]
%% reset  hashd-params           : [ Restore default hashd parameters ]
%% reset  sideloads              : [ Stop all sideloads ]
%% reset  sysloads               : [ Stop all sysloads ]
%% reset  resctl                 : [ Restore cpu/mem/io resource control ]
%% reset  resctl-params          : [ Restore default resource control parameters ]
%% reset  oomd                   : [ Restore default OOMD operation ]
%% reset  secondaries            : [ Reset sideloads + sysloads ]
%% reset  all-workloads          : [ Reset hashds + secondaries ]
%% reset  protections            : [ Reset resctl + oomd ]
%% reset  params                 : [ Reset hashd and resctl params ]
%% reset  all                    : [ All except params ]
%% reset  all-with-params        : [ All ]

Jump commands navigate across docs.

%% jump   index                  : * Exit to Index

When you leave this document, all states will be reset to default.

$$ reset all

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

Buttons toggle the toggles.

%% on     hashd                  : [ Start hashd ]
%% off    hashd                  : [ Stop hashd ]

%% toggle hashd                  : Toggle hashd
%% toggle bench-iocost           : Toggle iocost benchmark
%% toggle bench-hashd            : Toggle hashd benchmark
%% toggle bench-hashd-loop       : Toggle hashd benchmark loop
%% toggle hashd-B                : Toggle the second instance of hashd

A sideload is identified with the tag, "test-build" here. The following job
ID points to an entry in sideload-defs.json and determines the specific
workload.

%% toggle sideload compile-job build-linux-2x     : Toggle a 2x linux build sideload
%% toggle sideload compile-job-1 build-linux-32x  : Toggle a 32x linux build sideload
%% toggle sideload memory-hog memory-growth-50pct : Toggle a 50% memory growth sideload
%% toggle sideload memory-hog-1 memory-growth-1x  : Toggle a 1x memory growth sideload
%% toggle sideload memory-hog-hot memory-bloat-1x : Toggle a hot memory hog sideload
%% toggle sideload io-hog read-bomb               : Toggle a read hog sideload
%% toggle sideload cpu-hog burn-cpus-50pct        : Toggle a 50% cpu hog sideload
%% toggle sideload cpu-hog-1 burn-cpus-1x         : Toggle a 1x cpu hog sideload
%% toggle sideload cpu-hog-2 burn-cpus-2x         : Toggle a 2x cpu hog sideload

A sysload is a sideload which is run under system.slice without the
supervision of sideloader and can be used to illustrate oomd workload
protection or the need for sideloader.

%% toggle sysload compile-job build-linux-2x     : Toggle a 2x linux build sysload
%% toggle sysload compile-job-1 build-linux-32x  : Toggle a 32x linux build sysload
%% toggle sysload memory-hog memory-growth-50pct : Toggle a 50% memory growth sysload
%% toggle sysload memory-hog-1 memory-growth-1x  : Toggle a 1x memory growth sysload
%% toggle sysload memory-hog-hot memory-bloat-1x : Toggle a hot memory hog sysload
%% toggle sysload io-hog read-bomb               : Toggle a read hog sysload
%% toggle sysload cpu-hog burn-cpus-50pct        : Toggle a 50% cpu hog sysload
%% toggle sysload cpu-hog-1 burn-cpus-1x         : Toggle a 1x cpu hog sysload
%% toggle sysload cpu-hog-2 burn-cpus-2x         : Toggle a 2x cpu hog sysload
%%
%% toggle cpu-resctl             : Toggle CPU resource protection
%% toggle mem-resctl             : Toggle memory resource protection
%% toggle io-resctl              : Toggle IO resource protection
%% toggle oomd                   : Toggle OOMD
%% toggle oomd-work-mem-pressure : Toggle memory pressure protection in workload.slice
%% toggle oomd-work-senpai       : Toggle senpai in workload.slice
%% toggle oomd-sys-mem-pressure  : Toggle memory pressure protection in system.slice
%% toggle oomd-sys-senpai        : Toggle senpai in system.slice

A knob configures a value between 0.0 and 1.0. A knob command should either
have a value argument or prompt.

%% knob   hashd-load 0.6
%% knob   hashd-load             : Main workload load level                  :
%% knob   hashd-lat-target-pct   : Main workload latency target percentile   :
%% knob   hashd-lat-target       : Main workload latency target              :
%% knob   hashd-mem              : Main workload memory footprint            :
%% knob   hashd-file             : Main workload pagecache proportion        :
%% knob   hashd-file-max         : Main workload max pagecache proportion    :
%% knob   hashd-file-addr-stdev  : Main workload file access stdev           :
%% knob   hashd-anon-addr-stdev  : Main workload anon access stdev           :
%% knob   hashd-log-bps          : Main workload log write bandwidth         :
%% knob   hashd-weight           : Main workload weight                      :
%% knob   hashd-B-load           : Second workload load level                :
%% knob   hashd-B-lat-target-pct : Second workload latency target percentile :
%% knob   hashd-B-lat-target     : Second workload latency target            :
%% knob   hashd-B-mem            : Second workload memory footprint          :
%% knob   hashd-B-file           : Second workload pagecache proportion      :
%% knob   hashd-B-file-max       : Second workload max pagecache proportion  :
%% knob   hashd-B-file-addr-stdev: Second workload file access stdev         :
%% knob   hashd-B-anon-addr-stdev: Second workload anon access stdev         :
%% knob   hashd-B-log-bps        : Second workload log write bandwidth       :
%% knob   hashd-B-weight         : Second workload weight                    :
%%
%% knob   sys-cpu-ratio          : system CPU weight compared to workload    :
%% knob   sys-io-ratio           : system IO weight compared to workload     :
%% knob   mem-margin             : Memory for the rest of the system         :
%% knob   balloon                : Memory balloon size                       :
%% knob   cpu-headroom           : CPU headroom                              :

The main graph pane can view different graphs. For available graph tags, see
graph.rs::GraphTag.

%% graph  CpuUtil                : [ Show CPU utilization graph ]
%% graph  MemUtil                : [ Show memory utilization graph ]
%% graph  IoUtil                 : [ Show IO utilization graph ]
%% graph                         : [ Return to the default graph ]

Reset commands are shortcuts to restore to default configurations.

%% reset  benches                : [ Stop hashd and iocost benchmarks ]
%% reset  hashds                 : [ Stop hashd instances ]
%% reset  hashd-params           : [ Restore default hashd parameters ]
%% reset  sideloads              : [ Stop all sideloads ]
%% reset  sysloads               : [ Stop all sysloads ]
%% reset  resctl                 : [ Restore cpu/mem/io resource control ]
%% reset  resctl-params          : [ Restore default resource control parameters ]
%% reset  oomd                   : [ Restore default OOMD operation ]
%% reset  graph                  : [ Reset main graph view ]
%% reset  secondaries            : [ Reset sideloads + sysloads ]
%% reset  all-workloads          : [ Reset hashds + secondaries ]
%% reset  protections            : [ Reset resctl + oomd ]
%% reset  params                 : [ Reset hashd and resctl params ]
%% reset  all                    : [ All except params ]
%% reset  all-with-params        : [ All ]

Commands which don't need further inputs can be groups into a group which is
presented as a single button.

%% (                             : [ Reset params, start hashd and set load to 80% ]
%% reset  params
%% knob   hashd-load 0.8
%% on     hashd
%% )

Jump commands navigate across docs.

%% jump   index                  : * Exit to Index

When you leave this document, all states will be reset to default.

$$ reset all

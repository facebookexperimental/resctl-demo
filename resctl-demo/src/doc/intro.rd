## Copyright (c) Facebook, Inc. and its affiliates.
%% id intro                      : Welcome to resouce control demo
%% reset all

*Welcome*\n
*=======*

Hello, ___**pleasantries**___. The file format understands
paragraphs.

***[WARNING]*** Failed to meet %MissedSysReqs% system requirements. rd-agent is force
started but some demos won't behave as expected. Please visit the
following page for more details.

%% jump sysreqs                  : %MissedSysReqs%* System Requirements

The followings are what you can do.

%% on hashd                      : [ Start hashd ]
%% off hashd                     : [ Stop hashd ]
%%
%% knob hashd-load               : Adjust the load level: 
%%
%% toggle oomd                   : Tempt the fate by toggling OOMD
%% toggle oomd-work-mem-pressure : or by toggling workload mempressure protection
%% toggle oomd-work-senpai       : NOTICE ME SENPAI!

When you leave this page, everything will be shutdown.

%% jump intro.second             : * Next page
%% jump index                    : * Exit to index

##
## Shutdown everything
##
$$ reset all

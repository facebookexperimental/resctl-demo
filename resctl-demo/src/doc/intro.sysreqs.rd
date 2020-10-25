## Copyright (c) Facebook, Inc. and its affiliates.
%% id intro.sysreqs              : System Requirements

*System Requirements*\n
*===================*

 ___***WARNING***___: *Failed to meet %MissedSysReqs% system requirements.
 The failed requirements are marked* **red**. *rd-agent is force started but
 some demos won't behave as expected.*

Dividing resources among multiple workloads in a controlled manner requires
comprehensive control strategies that cover all resources—CPU, IO, and memory.

Many of the components and features required to implement these strategies are
new, and there are currently a substantial number of specific requirements:
These requirements are all listed on this page.

resctl-demo checks for all needed requirements and configures the ones it can,
but there are some requirements that can't be satisfied without system-level
changes.

As resource control becomes more widely adopted, the need for specific
configurations will diminish: Some will be adopted as standard practices, while
others become unnecessary as the underlying features grow more versatile.

If any of the requirements are unmet, you’ll see an error message at the top of
this page.

Each requirement is described below, along with its purpose, the automatic
configurations resctl-demo may apply, and info on meeting the requirement if
it's currently unmet:

* %SysReq::Controllers%: cgroup2 provides the foundation for resource control.
  resctl-demo requires the system to be managed by systemd using cgroup2 with
  all three major local resource controllers - cpu, memory and, io - enabled.

  If '/sys/fs/cgroup/cgroup.controllers' is not present, reboot the system with
  'systemd.unified_cgroup_hierarchy=1' specified as a boot parameter. If the
  file is present but doesn't contain all the controllers, either the kernel
  doesn't have them enabled, they're disabled with 'cgroup_disable' boot
  parameter, or cgroup1 hierarchies are using them. Resolve them and restart
  resctl-demo.

* %SysReq::Freezer%: cgroup2 freezer is used to strictly limit the impact of
  side workloads under heavy load. Available in kernels >= v5.2.

* %SysReq::MemCgRecursiveProt%: Recursive propagation for memory controller's
  memory.min/low protections. This greatly simplifies protection configurations.
  Available in kernels >= v5.6.

  It can be enabled with cgroup2 "memory_recursiveprot" mount option. If
  available, resctl-demo will automatically remount cgroup2 fs w/ the mount
  option. For details: https://lkml.org/lkml/2019/12/19/1272

* %SysReq::IoCost%: blk-iocost is the new IO controller which can
  comprehensively control IO capacity distribution proportionally. Enabled
  with CONFIG_BLK_CGROUP_IOCOST. For details:
  https://lwn.net/Articles/793460/

* %SysReq::IoCostVer%: blk-iocost received significant updates to improve
  control quality and visibility during the v5.10 development cycle. A
  kernel with these updates is recommended. For details:
  https://lwn.net/Articles/830397/

* %SysReq::NoOtherIoControllers%: Other IO controllers - io.max and io.latency -
  can interfere and shouldn't have active configurations.

  If configured through systemd, remove all IO{Read|Write}{Bandwidth|IOPS}Max
  and IoDeviceLatencyTargetSec configurations.

* %SysReq::AnonBalance%: Kernel memory management received a major update
  during the v5.8 development cycle which put anonymous memory on an equal
  footing with page cache and made swap useful, especially on SSDs. For
  details: https://lwn.net/Articles/821105/

* %SysReq::Btrfs%: Working IO isolation requires support from filesystem to
  avoid priority inversions. Currently, btrfs is the only supported filesystem.

  The OS must be installed with btrfs as the root filesystem.

* %SysReq::BtrfsAsyncDiscard%: Many SSDs show significant latency spikes when
  discards are issued in bulk, which can lead to severe priority inversions.
  Async discard is a btrfs feature that paces and reduces the total amount of
  discards.

  It can be enabled with "discard=async" mount option on kernels >= v5.6. If
  available, resctl-demo will automatically remount the filesystem with the mount
  option. For details: https://lwn.net/Articles/805300/

* %SysReq::NoCompositeStorage%: Currently, composite block devices, such as
  dm and md, break the chain of custody for IOs, allowing cgroups to escape
  IO control and cause severe priority inversions.

  The filesystem must be on a physical device.

* %SysReq::IoSched%: bfq IO scheduler's implementation of proportional IO
  control conflicts with blk-iocost and breaks IO isolation. Use
  mq-deadline.

  IO scheduler can be selected by writing to /sys/block/$DEV/queue/scheduler.
  resctl-demo automatically switches to mq-deadline if available.

* %SysReq::NoWbt%: Write-Back-Throttling is a block layer mechanism to prevent
  writebacks from overwhelming IO devices. This may interfere with IO control
  and should be disabled.

  It can be disabled by writing 0 to /sys/block/$DEV/queue/wbt_lat_usec.
  resctl-demo automatically disables wbt.

* %SysReq::Swap%: Swap must be enabled with the default swappiness and at
  least as large as the smaller of a third of the system memory, or 32G.

  See %SysReq::SwapOnScratch%.

* %SysReq::SwapOnScratch%: Swap must be on the same device as the root
  filesystem. The recommended configuration is btrfs root filesystem, which
  serves both the scratch directory and swap file. This isn't an inherent
  requirement of resource control but exists to simplify experiments.

  Setting up btrfs swapfiles:
  https://wiki.archlinux.org/index.php/Btrfs#Swap_file

* %SysReq::Oomd%: OOMD binary >= 0.3.0 && != 0.4.0 must be present. Note
  that 0.4.0 is excluded due to a bug in Senpai implementation. See
  https://github.com/facebookincubator/oomd.

* %SysReq::NoSysOomd%: Instances of OOMD or earlyoom at the system-level may
  interfere and should be disabled. They usually run as a systemd service of
  the same name. You can use `systemctl` to locate and stop the services.

  Disable system-level OOMD and earlyoom services. resctl-demo automatically
  stops and restarts system-level OOMD instance.

* %SysReq::HostCriticalServices%: sshd.service, systemd-journald.service,
  dbus.service, dbus-broker.service must be in ___hostcritical___.

  resctl-demo automatically creates the needed configurations but for the
  changes to take effect, either the machine or services need to be
  restarted.

* %SysReq::Dependencies%: 'python3', 'findmnt', 'dd', 'fio', 'stdbuf',
  'gcc', 'ld', 'make', 'bison', 'flex', 'pkg-config', 'stress', 'libssl' and
  'libelf' must be available on the system.

  Install the needed packages.

%% jump intro.iocost             : [ Next: Iocost Parameters and Benchmark ]

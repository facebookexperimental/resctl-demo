## Copyright (c) Facebook, Inc. and its affiliates.
%% id intro.sysreqs              : System Requirements

*System Requirements*\n
*===================*

  ***(WARNING)*** *Failed to meet %MissedSysReqs% system requirements. The failed
  requirements are marked* **red**. *rd-agent is force started but some demos
  won't behave as expected.*

Dividing up resource usages for multiple workloads in a controlled manner
requires comprehensive resource control strategies for all resources. Many of
the required features are new and some of them have specific requirements. As
resource control becomes more widely adopted, the need for specific
configurations will go away. Some will be adopted as standard practices while
others become unnecessary as the underlying features grow more versatile.

For now, we have a ton of requirements to meet to achieve comprehensive resource
control. resctl-demo checks for all needed requirements and configures the ones
that it can but there are requirements which can not be satisfied without
system-level changes. Each requirement is listed below along with why it's
there, what automatic configurations resctl-demo may apply and how to resolve if
**unmet**.

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

* %SysReq::IoCost%: blk-iocost is the new IO controller which can
  comprehensively control IO capacity distribution proportionally. Available in
  kernels >= v5.4 (CONFIG_BLK_CGROUP_IOCOST). For more details:
  https://lwn.net/Articles/793460/

* %SysReq::NoOtherIoControllers%: Other IO controllers - io.max and io.latency -
  can interfere and shouldn't have active configurations.

  If configured through systemd, remove all IO{Read|Write}{Bandwidth|IOPS}Max
  and IoDeviceLatencyTargetSec configurations.

* %SysReq::Btrfs%: Working IO isolation requires support from filesystem to
  avoid priority inversions. Currently, btrfs is the only supported filesystem.

  The OS must be installed with btrfs as the root filesystem.

* %SysReq::BtrfsAsyncDiscard%: Many SSDs show significant latency spikes when
  discards are issued in bulk which can lead to severe priority inversions.
  Async discard is a btrfs feature which reduces the total amount of and paces
  discards.

  It can be enabled with "discard=async" mount option on kernels >= v5.6. If
  available, resctl-demo will automatically remount the filesystem w/ the mount
  option. For more details: https://lwn.net/Articles/805300/

* %SysReq::NoCompositeStorage%: Currently, composite block devices such as dm
  and md break the chain of custody for IOs allowing cgroups to escape IO
  control and cause severe priority inversions.

  The filesystem must be on a physical device.

* %SysReq::IoSched%: bfq's implementation of proportional IO control conflicts
  with blk-iocost and breaks IO isolation. Use mq-deadline.

  IO scheduler can be selected by writing to /sys/block/$DEV/queue/scheduler.
  resctl-demo automatically switches to mq-deadline if available.

* %SysReq::NoWbt%: Write-Back-Throttling is a block layer mechanism to prevent
  writebacks from overwhelming IO devices. This may interfere with IO control
  and should be disabled.

  It can be disabled by writing 0 to /sys/block/$DEV/queue/wbt_lat_usec.
  resctl-demo automatically disables wbt.

* %SysReq::Swap%: Swap must be enabled with the default swappiness and at least
  as large as memory.

  See %SysReq::SwapOnScratch%.

* %SysReq::SwapOnScratch%: Swap must be on the same device as the root
  filesystem. The recommended configuration is btrfs root filesystem which
  serves both the scratch directory and swap file. This isn't an inherent
  requirement of resource control but exists to simplify experiments.

  Setting up btrfs swapfiles:
  https://wiki.archlinux.org/index.php/Btrfs#Swap_file

* %SysReq::NoSysOomd%: Instances of oomd or earlyoom at the system-level may
  interfere and should be disabled.

  Disable system-level oomd and earlyoom services. resctl-demo automatically
  stops and restarts system-level oomd instance.

* %SysReq::HostCriticalServices%: sshd.service, systemd-journald.service,
  dbus.service, dbus-broker.service must be in hostcritical.slice.

  resctl-demo automatically creates the needed configurations but for the
  changes to take effect either the machine or services need to be restarted.

* %SysReq::Dependencies%: 'python3', 'findmnt', 'dd', 'fio', 'stdbuf', 'gcc',
  'ld', 'make', 'bison', 'flex', 'pkg-config', 'nproc', 'libssl' and 'libelf'
  must be available on the system.

  Install the needed packages.

%% jump intro                    : [ Go back to intro ]
%% jump index                    : [ Exit to index ]

## Copyright (c) Facebook, Inc. and its affiliates.
%% id intro.sysreqs              : System Requirements

*System Requirements*\n
*===================*

*Failed to meet %MissedSysReqs% system requirements. The failed requirements are
marked* **red**. *rd-agent is force started but some demos won't behave as expected.*

* %SysReq::Controllers%: cgroup2 provides the foundation for resource control.
  resctl-demo requires the system to be managed by systemd using cgroup2 with
  all three major local resource controllers - cpu, memory and, io - available.

  If '/sys/fs/cgroup/cgroup.controllers' is not present, reboot the system with
  'systemd.unified_cgroup_hierarchy=1' specified as a boot parameter. If the
  file is present but doesn't contain all the controllers, either the kernel
  doesn't have them enabled, they're disabled with 'cgroup_disable' boot
  parameter, or cgroup1 hierarchies are using them. Resolve them and restart
  resctl-demo.

* %SysReq::Freezer%: cgroup2 freezer is used to strictly limit the impact of
  side workloads under heavy load. Available in kernels >= v5.2.

* %SysReq::IoCost%: blk-iocost is the new IO controller which can control IO
  capacity distribution proportionally and fully and required for comprehensive
  resource isolation. Available in kernels >= v5.4. For more details:
  https://lwn.net/Articles/793460/

* %SysReq::NoOtherIoControllers%: Other IO controllers - io.max, io.latency or
  bfq - can interfere and should be disabled.

* %SysReq::Btrfs%: Working IO isolation requires support from filesystem to
  avoid priority inversions. Currently, btrfs is the only supported filesystem.
  The recommended configuration is single btrfs instance on a physical device.

* %SysReq::BtrfsAsyncDiscard%: Many SSDs show significant latency spikes when
  discards are issued in bulk which can lead to severe priority inversion
  issues. Async discard is a btrfs feature which reduces the total amount of and
  paces discards. It can be enabled with "discard=async" mount option on kernels
  >= v5.6. For more details: https://lwn.net/Articles/805300/

* %SysReq::NoCompositeStorage%: Currently, composite block devices such as dm
  and md break the chain of custody for IOs allowing cgroups to escape IO
  control and cause severe priority inversions. The filesystem should be on a
  physical device.

* %SysReq::NoWbt%: Write-back-throttling is a block layer mechanism to prevent
  writebacks from overwhelming IO devices. This may interfere with IO control
  and should be disabled.

* %SysReq::Swap%: Swap should be enabled with the default swappiness and at
  least as large as memory. The recommended configuration is single btrfs
  instance on a physical device with a big enough swapfile on it. For more
  details: https://wiki.archlinux.org/index.php/Btrfs#Swap_file

* %SysReq::SwapOnScratch%: Swap should be on the same device as the scratch
  filesystem. The recommended configuration is single btrfs instance on a
  physical device with a big enough swapfile on it. For more details:
  https://wiki.archlinux.org/index.php/Btrfs#Swap_file

* %SysReq::NoSysOomd%: Instances of oomd or earlyoom at the system level may
  interfere and should be disabled.

* %SysReq::HostCriticalServices%: sshd.service, systemd-journald.service,
  dbus.service, dbus-broker.service must be in hostcritical.slice. rd-agent
  creates the needed configurations on startup but for the changes to take
  effect either the machine needs a reboot or services restarts.

* %SysReq::Dependencies%: 'python3', 'findmnt', 'dd', 'fio' and 'stdbuf' must be
  available.

%% jump intro                    : [ Go back to intro ]
%% jump index                    : [ Exit to index ]

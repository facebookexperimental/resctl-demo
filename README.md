# Facebook resctl-demo

Resource control aims to control compute resource distribution to improve
reliability and utilization of a system. The facebook kernel and container teams
have been intensively researching and implementing mechanisms and methods to
advance resource control. resctl-demo demonstrates and documents various aspects
of resource control using self-contained workloads in guided scenarios.

## Requirements

The basic building blocks are provided by the Linux kernel's cgroup2 and other
resource related features. On top, usage and configuration methods combined with
user-space helpers such as oomd and sideloader implement resource isolation to
achieve workload protection and stacking.

* Linux kernel >= v5.6

* cgroup2

* btrfs on non-composite storage device (sda or nvme0n1, not md or dm)

* Swap file on btrfs at least as large as physical memory

* systemd

* oomd

* python3, findmnt, dd, fio, stdbuf

## Building

```
$ cargo build --release
```

## Installing resctl-demo

```
$ sudo ./install.sh
```

## Running resctl-demo

```
$ sudo systemd-run --scope --slice hostcritial.slice --unit resctl-demo /usr/local/bin/resctl-demo
```

## License

resctl-demo is apache-2.0 licensed, as found in the LICENSE file.

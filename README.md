<img src="img/logo.svg" alt="resctl-demo logo" width="50%"/>

Resource control aims to control compute resource distribution to improve
reliability and utilization of a system. The facebook kernel and container
teams have been intensively researching and implementing mechanisms and
methods to advance resource control. This repository contains two projects -
resctl-demo and resctl-bench.

resctl-demo
-----------

resctl-demo demonstrates and documents various aspects of resource control
using self-contained workloads in guided scenarios.

<a href="https://engineering.fb.com/wp-content/uploads/2020/10/resctl-demoV2.mp4">
  <img src="img/screenshot.png" alt="resctl-demo in action" width="50%">
</a>

resctl-bench
------------

resctl-bench is a collection of whole-system benchmarks to evalute resource
control and hardware behaviors using realistic simulated workloads.

Comprehensive resource control involves the whole system. Furthermore,
testing resource control end-to-end requires scenarios involving realistic
workloads and monitoring their interactions. The combination makes
benchmarking resource control challenging and error-prone. It's easy to slip
up on a configuration and testing with real workloads can be tedious and
unreliable.

resctl-bench encapsulates the whole process so that resource control
benchmarks can be performed easily and reliably. It verifies and updates
system configurations, reproduces resource contention scenarios with a
realistic latency-sensitive workload simulator and other secondary
workloads, analyzes the resulting system and workload behaviors, and
generates easily understandable reports.

Read the [documentation](resctl-bench/README.md) for more information.


Premade System Images
=====================

Comprehensive resource control has many requirements, some of which can be
difficult to configure on an existing system. resctl-demo provides premade
images to help getting started. Visit the following page for details:

  https://facebookmicrosites.github.io/resctl-demo-website


Installation
============

resctl-demo and resctl-bench can be installed using `cargo install`. Don't
forget to install rd-hashd and rd-agent.

```
cargo install rd-hashd rd-agent resctl-demo resctl-bench
```

The followings are commands to install other dependencies on different
distros.


arch
----

The common dependencies:
```
sudo pacman -S --needed coreutils util-linux python fio
```

oomd is available through AUR:
```
sudo pacman -S --needed fakeroot
git clone https://aur.archlinux.org/oomd-git.git oomd-git
cd oomd-git
makepkg -si
```

resctl-demo needs the followings to plot graphs and run linux build job as
one of the workloads:
```
sudo pacman -S --needed gnuplot gcc binutils make bison flex pkgconf stress openssl libelf
```


fedora
------

The common dependencies:
```
yum install coreutils util-linux python3 fio oomd
```

resctl-demo needs the followings to plot graphs and run linux build job as
one of the workloads:
```
yum install gnuplot gcc binutils make bison flex pkgconf stress openssl-devel elfutils-devel
```


Building and Installing Manually
================================

Building is straight-forward. Check out the source code and run:

```
cargo build --release
```

Installing from local source directory:

```
cargo install --path rd-hashd
cargo install --path rd-agent
cargo install --path resctl-demo
cargo install --path resctl-bench
```

Alternatively, run `build-and-tar.sh` script to create a tarball containing
the binaries:

```
./build-and-tar.sh
```

You can install resctl-demo and resctl-bench by simply untarring the
resulting tarball:

```
cd /usr/local/bin
tar xvzf $SRC_DIR/target/resctl-demo.tar.gz
```

Follow the instructions in the Installation section to install other
dependencies.


Running resctl-demo
===================

resctl-demo should be run as root in hostcritical.slice. Use the following
command:

```
sudo systemd-run --scope --slice hostcritical.slice --unit resctl-demo /usr/local/bin/resctl-demo
```


Requirements
============

The basic building blocks are provided by the Linux kernel's cgroup2 and other
resource related features. On top, usage and configuration methods combined with
user-space helpers such as oomd and sideloader implement resource isolation to
achieve workload protection and stacking.

* Linux kernel in the git branch
  `https://git.kernel.org/pub/scm/linux/kernel/git/tj/misc.git
  resctl-demo-v5.13-rc7` which contains the following extra commits on top
  of v5.13-rc7:
    * Four mm commits to [fix inode shadow entry
      protection](resctl-bench/doc/shadow-inode.md)
    * Backport of [`blkcg: drop CLONE_IO check in
      blkcg_can_attach()`](https://git.kernel.org/pub/scm/linux/kernel/git/axboe/linux-block.git/commit/?h=for-5.14/block&id=b5f3352e0868611b555e1dcb2e1ffb8e346c519c)
* cgroup2
* btrfs on non-composite storage device (sda or nvme0n1, not md or dm)
* Swap file on btrfs at least as large as 1/3 of physical memory
* systemd
* oomd
* dd, stdbuf, findmnt, python3, fio, stress, gnuplot, gcc, ld, make, bison,
  flex, pkg-config, libssl, libelf


License
=======

resctl-demo is apache-2.0 licensed, as found in the [LICENSE](LICENSE) file.

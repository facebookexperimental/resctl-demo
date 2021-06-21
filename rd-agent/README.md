
# Resource-control demo agent

`rd-agent` orchestrates resource control demo and benchmark scenarios
end-to-end. It runs benchmarks to establish the baseline, manages `rd-hashd`
instances as the primary workloads, simulates resource conflicts with other
workloads, and monitors the system and workloads to generate detailed
reports.

Comprehensive resource control requires a number of components closely
working together. `rd-agent` will check all the needed features and try to
configure the system as necessary, and report all the missing pieces. The
following basic system configuration is expected.

* The root filesystem must be btrfs and on a physical device (not md or dm).

* Swap must be on the same device as root filesystem larger than half the
  memory. Swapfile on the root filesystem is preferred.

* The scratch directory must be on the root filesystem.

* `systemd` is the system agent and using cgroup2.

Some of the system configuration failures can be ignored with `--force`.
However, resource isolation may not work as expected.

Configurations, commanding and reporting happen through json files under
`/var/lib/resctl-demo` by default. All files used by workloads are under the
`scratch` sub-directory. Take a look at `index.json` and `cmd.json` if you
want to explore the control files.

`rd-agent` is usually used as a part of `resctl-demo` or `resctl-bench`. For
more information on the containing projects, visit:

  https://github.com/facebookexperimental/resctl-demo

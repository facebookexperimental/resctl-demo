
# Resource Control Demo

Resource control aims to control compute resource distribution to improve
reliability and utilization of a system. The facebook kernel and container
teams have been intensively researching and implementing mechanisms and
methods to advance resource control. `resctl-demo` demonstrates and
documents various aspects of resource control using self-contained workloads
in guided scenarios. Here's a screencast:

  https://engineering.fb.com/wp-content/uploads/2020/10/resctl-demoV2.mp4


# Getting Started

Comprehensive resource control has many requirements, some of which can be
difficult to configure on an existing system. `resctl-demo` provides premade
images to help getting started. Visit the following page for details:

  https://facebookmicrosites.github.io/resctl-demo-website

For other installation options, visit:

  https://github.com/facebookexperimental/resctl-demo

Once you're ready, start exploring:

```
$ sudo systemd-run --scope --unit resctl-demo --slice hostcritical.slice resctl-demo
```

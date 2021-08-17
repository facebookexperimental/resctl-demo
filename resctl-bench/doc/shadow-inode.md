Inode Shadow Entry Protection
=============================

The Linux kernel uses inode shadow entries to remember whether memory pages
from a given file were on memory but evicted due to memory shortage. If one
of the pages is needed again, the page fault is considered to be a refault
which could have been avoided had there been more memory available. This
distinction decides whether a given cgroup is under memory shortage which in
turn affects memory pressure calculation and, crucially, whether memory.low
protection should kick in.

Unfortunately, in the current upstream kernels, the way inodes are reclaimed
can make the kernel lose this information prematurely:

* The kernel can decide to reclaim an inode data structure regardless of how
  many pages are currently attached to it. When the inode is reclaimed, all
  the pages and the record of them being on memory are gone.

* When all memory pages that are attached to an inode are reclaimed, the
  inode itself is reclaimed losing the memory residency information for all
  its pages. Depending on the usage pattern and hardware characteristics,
  the above conditions can trigger frequently causing the kernel to believe
  that a cgroup is not experiencing memory shortage while the cgroup, in
  reality, is under extreme memory pressure, completely voiding memory
  protection through memory.low and memor.min.

There is a pending solution which is scheduled to be merged for v5.15:

  https://lore.kernel.org/linux-fsdevel/20210614211904.14420-4-hannes@cmpxchg.org/

The following git branch contains v5.14-rc6 + the proposed patches which can
be used in the meantime:

  https://git.kernel.org/pub/scm/linux/kernel/git/tj/misc.git resctl-demo-v5.14-rc6

Note that the above branch contains a patch which tags the kernel as having
shadow inode protection. Without the tagging, on kernels < v5.15,
resctl-bench doesn't have a quick and reliable way to tell whether shadow
inodes are protected and tries to test inode protection with a benchmark
which can take several minutes. Unfortunately, the test isn't completely
reliable and may occasionally produce incorrect results. It is recommended
to upgrade to the kernel >= v5.15 or apply the tagging patch.

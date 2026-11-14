# Built-in items

Items collected by the Cerbrix agent. Items without a parameter are reported once per host; `disk.*` and `net.*` items are reported once per mount point or interface.

| Key | Unit | Parameters | Meaning |
|---|---|---|---|
| `cpu.usage` | percent (0-100) | none | CPU usage averaged over all cores. |
| `system.load1` | load | none | Load average over the last 1 minute. |
| `system.load5` | load | none | Load average over the last 5 minutes. |
| `system.load15` | load | none | Load average over the last 15 minutes. |
| `memory.total_bytes` | bytes | none | Total physical memory. |
| `memory.used_bytes` | bytes | none | Physical memory in use. |
| `memory.available_bytes` | bytes | none | Physical memory available to applications without swapping. |
| `memory.used_percent` | percent (0-100) | none | `memory.used_bytes` as a share of `memory.total_bytes`. |
| `swap.total_bytes` | bytes | none | Total swap space. |
| `swap.used_bytes` | bytes | none | Swap space in use. |
| `swap.used_percent` | percent (0-100) | none | `swap.used_bytes` as a share of `swap.total_bytes`. Not reported when the host has no swap. |
| `system.uptime_seconds` | seconds | none | Time since the host booted. |
| `disk.total_bytes` | bytes | `mount` | Total size of the file system mounted at `mount`. |
| `disk.used_bytes` | bytes | `mount` | Space in use on the file system (total minus available). |
| `disk.used_percent` | percent (0-100) | `mount` | `disk.used_bytes` as a share of `disk.total_bytes`. |
| `net.rx_bytes_per_sec` | bytes per second | `interface` | Received data rate of the network interface. |
| `net.tx_bytes_per_sec` | bytes per second | `interface` | Transmitted data rate of the network interface. |

`cpu.usage` is measured between two consecutive collections: it is the average
CPU usage during the interval since the previous collection. The first
collection after the agent starts has nothing to compare against and produces
no value, so the item appears one collection interval after start-up. Load
averages are not available on Windows; the agent reports them as 0 there.

Mounts and network interfaces are discovered on every collection, so ones that
appear or disappear while the agent runs are picked up without a restart.

`disk.*` items are reported for every mount except file systems whose type is
in the exclusion list (compared case-insensitively) and mount points that match
an excluded mount pattern. The default excluded file-system types are:
`proc`, `sysfs`, `devtmpfs`, `devpts`, `tmpfs`, `cgroup`, `cgroup2`, `overlay`,
`squashfs`, `autofs`, `debugfs`, `tracefs`, `securityfs`, `pstore`, `bpf`,
`mqueue`, `hugetlbfs`, `fusectl`, `configfs`, `ramfs` and `nsfs`. A mount point
listed twice is reported once, and file systems with a total size of 0 are
skipped. A device mounted at several points (bind mounts) is reported once, at its
shortest mount point.

`net.*` rates are measured between two consecutive collections, so the first
collection after start-up produces no value, and an interface whose counters
went backwards (for example after a reset) produces none for that round.
Loopback interfaces (`lo`, `lo0`, and Windows "Loopback" pseudo-interfaces)
are always excluded, as are interfaces matching an excluded interface pattern.

Exclusion patterns support `*` (any run of characters, including `/`) and `?`
(exactly one character), match the whole name and are case-sensitive.

Mount points and interface names whose value is not a valid parameter value
(empty, longer than 256 bytes, or containing control characters) are skipped
with a warning.

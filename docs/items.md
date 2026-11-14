# Built-in items

Items collected by the Cerbrix agent. None of these items takes parameters.

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

`cpu.usage` is measured between two consecutive collections: it is the average
CPU usage during the interval since the previous collection. The first
collection after the agent starts has nothing to compare against and produces
no value, so the item appears one collection interval after start-up. Load
averages are not available on Windows; the agent reports them as 0 there.

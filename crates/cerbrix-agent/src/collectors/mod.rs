//! Built-in collectors.

pub mod system;

pub use system::{CpuCollector, LoadCollector, MemoryCollector, UptimeCollector};

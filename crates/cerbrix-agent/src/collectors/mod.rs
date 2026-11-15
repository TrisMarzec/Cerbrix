//! Built-in collectors.

pub mod disk;
pub mod filter;
pub mod net;
pub mod select;
pub mod system;

pub use disk::{DEFAULT_EXCLUDED_FS, DiskCollector};
pub use filter::Patterns;
pub use net::NetCollector;
pub use select::FilteredCollector;
pub use system::{CpuCollector, LoadCollector, MemoryCollector, UptimeCollector};

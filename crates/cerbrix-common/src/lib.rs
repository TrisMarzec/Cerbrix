//! Wire types and validation shared by the Cerbrix agent, proxy and server.

pub mod agent_config;
pub mod config;
mod enroll;
mod error;
pub mod interval;
pub mod items;
mod metric;

pub use agent_config::{ConfiguredItem, ItemConfig};
pub use config::host::{HostFile, ItemEntry, NotificationFile, TemplateFile, TriggerEntry};
pub use config::{Severity, validate_name};
pub use enroll::{EnrollRequest, EnrollResponse, validate_host_name};
pub use error::{ErrorBody, ValidationError};
pub use interval::{Interval, IntervalError};
pub use items::{BUILTIN_ITEMS, ItemSpec, builtin_item, default_agent_items};
pub use metric::{
    IngestResponse, MAX_BATCH_VALUES, MAX_ITEM_KEY_LEN, MAX_PARAM_VALUE_LEN, MAX_PARAMS,
    MetricValue, PROTOCOL_VERSION, ValueBatch, validate_item_key, validate_param_name,
};

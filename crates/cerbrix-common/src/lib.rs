//! Wire types and validation shared by the Cerbrix agent, proxy and server.

mod enroll;
mod error;
mod metric;

pub use enroll::{EnrollRequest, EnrollResponse, validate_host_name};
pub use error::{ErrorBody, ValidationError};
pub use metric::{
    IngestResponse, MAX_BATCH_VALUES, MAX_ITEM_KEY_LEN, MAX_PARAM_VALUE_LEN, MAX_PARAMS,
    MetricValue, PROTOCOL_VERSION, ValueBatch, validate_item_key, validate_param_name,
};

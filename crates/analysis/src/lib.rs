//! Project-local DuckDB over immutable Parquet measurements. No scan-target writes.
#![deny(unsafe_op_in_unsafe_fn)]
mod duck;
mod store;
pub use store::{compact_response, encode_response, Store, DEFAULT_LIMIT, OUTPUT_BUDGET, FULL_HISTORY_BUDGET};

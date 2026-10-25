//! Local metadata analysis. No scan-target writes, file-content reads or network.
#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(not(target_os = "macos"))]
compile_error!("The first Macscan backend requires macOS.");

mod compare;
mod foundation;
mod platform;
mod query;
mod scan;
mod store;
mod volume;

pub use compare::CompareQuery;
pub use query::{Metric, Query, QueryKind, OUTPUT_BUDGET};
pub use scan::{
    capabilities, scan, scan_with_progress, Entry, EntryKind, Issue, Progress, Scan, ScanOptions,
    ScanStatus,
};
pub use store::{Store, STORE_BUDGET};
pub use volume::volumes;

pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SCHEMA_VERSION: &str = "1.0";

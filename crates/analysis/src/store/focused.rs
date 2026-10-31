//! Saved identities only: preparation reads immutable metadata, never live targets.
use super::*;
use macscan_engine::{Progress, ScanOptions};
use std::os::unix::ffi::OsStringExt;

impl Store {
    pub fn prepare_rescan(&self, source_scan_id: &str, source_entry_id: u32) -> io::Result<Value> {
        let manifest = self.manifest(source_scan_id)?;
        let report = self.report(
            source_scan_id,
            &Query {
                entry: source_entry_id,
                kind: QueryKind::Inspect,
                ..Query::default()
            },
            &AtomicBool::new(false),
        )?;
        let row = report["records"]
            .get(0)
            .ok_or_else(|| invalid("saved entry not found"))?;
        if !matches!(row["kind"].as_str(), Some("directory" | "file")) {
            return Err(invalid(
                "Genmåling kræver en gemt mappe eller almindelig fil",
            ));
        }
        let root = focused_path(row)?;
        if !root.is_absolute() {
            return Err(invalid("saved path must be absolute"));
        }
        Ok(
            json!({"source_scan_id":source_scan_id,"source_entry_id":source_entry_id.to_string(),
            "source_started_at":manifest["started_at"],"root_display":root.to_string_lossy(),
            "root_path_bytes_hex":row["path_bytes_hex"],"kind":row["kind"]}),
        )
    }

    pub fn rescan_to_store(
        &self,
        source_scan_id: &str,
        source_entry_id: u32,
        options: &ScanOptions,
        budget: u64,
        cancel: &AtomicBool,
        progress: impl FnMut(&Progress),
    ) -> io::Result<String> {
        // Resolve again at explicit start; client display paths are never input.
        let prepared = self.prepare_rescan(source_scan_id, source_entry_id)?;
        let raw = unhex(
            prepared["root_path_bytes_hex"]
                .as_str()
                .ok_or_else(|| invalid("missing raw path"))?,
        )?;
        let root = PathBuf::from(std::ffi::OsString::from_vec(raw));
        let origin = json!({"source_scan_id":source_scan_id,"source_entry_id":source_entry_id.to_string(),
            "source_started_at":prepared["source_started_at"]});
        self.scan_to_store_origin(&root, options, budget, cancel, Some(origin), progress)
    }
}
fn focused_path(row: &Value) -> io::Result<PathBuf> {
    let raw = unhex(
        row["path_bytes_hex"]
            .as_str()
            .ok_or_else(|| invalid("missing saved raw path"))?,
    )?;
    if raw.contains(&0) {
        return Err(invalid("NUL in saved path"));
    }
    Ok(PathBuf::from(std::ffi::OsString::from_vec(raw)))
}

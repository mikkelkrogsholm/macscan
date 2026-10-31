//! Whole-measurement event accounting. Counts never depend on retained examples.
use serde_json::{json, Value};
use std::io;
pub const REASONS: [&str; 9] = [
    "access_denied",
    "provider_excluded",
    "volume_boundary",
    "firmlink_boundary",
    "scanner_output",
    "disappeared",
    "explicit_limit",
    "io_error",
    "volume_changed",
];
pub fn empty() -> Value {
    let counts: serde_json::Map<String, Value> =
        REASONS.iter().map(|r| ((*r).into(), json!(0))).collect();
    json!({"version":1,"quality":"event_counts","unit":"events_not_unique_areas","counts":counts,"samples":[],"sample_limit_per_reason":2,"samples_truncated":false})
}
pub fn bounded(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].into()
}
pub fn reason(code: &str, error: Option<&io::Error>) -> &'static str {
    if let Some(e) = error {
        return match e.kind() {
            io::ErrorKind::PermissionDenied => "access_denied",
            io::ErrorKind::NotFound => "disappeared",
            _ => "io_error",
        };
    }
    match code {
        "CLOUD_EXCLUDED" => "provider_excluded",
        "MOUNT_EXCLUDED" => "volume_boundary",
        "FIRMLINK_EXCLUDED" => "firmlink_boundary",
        "OUTPUT_EXCLUDED" => "scanner_output",
        "SCAN_LIMIT" | "DEPTH_LIMIT" | "MEMORY_LIMIT" | "EXPLICIT_EXCLUDED" => "explicit_limit",
        "VOLUME_CHANGED" => "volume_changed",
        _ => "io_error",
    }
}
pub fn record(
    d: &mut Value,
    id: u32,
    code: &str,
    operation: &str,
    detail: &str,
    error: Option<&io::Error>,
    name: Option<&[u8]>,
) {
    if d["version"] != 1 {
        *d = empty();
    }
    let r = reason(code, error);
    let n = d["counts"][r].as_u64().unwrap_or(0);
    d["counts"][r] = json!(n.saturating_add(1));
    d["samples_truncated"]=json!(d["samples_truncated"]==true || n>=2);
    let samples = d["samples"].as_array_mut().unwrap();
    if samples.iter().filter(|s| s["reason"] == r).count() < 2 {
        // Darwin direct-child names are <=255 bytes. Longer synthetic names retain
        // parent context only, explicitly, rather than inventing a child path.
        let child = name.filter(|n| n.len() <= 255);
        samples.push(json!({"entry_id":id.to_string(),"reason":r,"code":code,"operation":operation,
            "detail":bounded(detail,160),"detail_truncated":detail.len()>160,
            "io_kind":error.map(|e|format!("{:?}",e.kind())),"errno":error.and_then(|e|e.raw_os_error()),
            "child_name_bytes_hex":child.map(super::scan::hex),"child_identity_truncated":name.is_some()&&child.is_none(),
            "path_context":if child.is_some(){"saved_parent_and_raw_child"}else{"saved_entry"}}));
    }
}
pub fn next_step(reason: &str) -> &'static str {
    match reason {
        "access_denied" => "Kontrollér appens adgang i macOS, og mål igen.",
        "provider_excluded" => "Kontrollér området i udbyderens app; denne måling udelader det.",
        "volume_boundary" | "firmlink_boundary" => "Åbn målingen for det relevante volumen.",
        "scanner_output" => "Scannerens egne metadata udelades med vilje.",
        "disappeared" => "Stien ændrede sig under målingen; mål igen.",
        "explicit_limit" => "Kontrollér de valgte udeladelser eller grænser, og mål igen.",
        "volume_changed" => "Volumenets identitet ændrede sig; mål hele området igen.",
        _ => "Kontrollér den gemte fejl og stiens tilgængelighed, og mål igen.",
    }
}

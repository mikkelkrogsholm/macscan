//! Command-local descriptions also gate parser option applicability.
use serde_json::{json, Value};

pub const COMMANDS: &[&str] = &[
    "schema",
    "capabilities",
    "volumes",
    "scan",
    "scans",
    "summary",
    "children",
    "largest",
    "inspect",
    "insights",
    "profile",
    "query",
    "compare",
    "growth",
    "coverage",
    "history-prune",
    "--version",
];

pub fn options(command: &str) -> Vec<(&'static str, &'static str)> {
    let mut flags = vec![(
        "--format",
        "json|ndjson; default json; NDJSON has start/record/end envelopes",
    )];
    if command == "schema" {
        flags.push((
            "--command",
            "installed command name; omit for full machine schema",
        ));
    }
    if matches!(
        command,
        "scan"
            | "scans"
            | "summary"
            | "children"
            | "largest"
            | "inspect"
            | "insights"
            | "profile"
            | "query"
            | "compare"
            | "growth"
            | "coverage"
            | "history-prune"
    ) {
        flags.push(("--store", "absolute directory, no dot/parent components; default $HOME/Library/Application Support/Macscan"));
    }
    if matches!(
        command,
        "summary"
            | "children"
            | "largest"
            | "inspect"
            | "insights"
            | "profile"
            | "query"
            | "coverage"
    ) {
        flags.push((
            "--scan",
            "required saved snapshot/volume-scan ID string from scan/scans; not a whole-Mac run ID",
        ));
    }
    if matches!(
        command,
        "scan"
            | "summary"
            | "children"
            | "largest"
            | "inspect"
            | "insights"
            | "profile"
            | "compare"
            | "growth"
    ) {
        flags.push(("--metric", "logical|allocated|apfs-data-clone; default allocated; private unsupported; DATA correction requires captured metadata and is not unique/private/reclaimable space"));
    }
    if matches!(
        command,
        "summary" | "children" | "largest" | "inspect" | "insights" | "profile" | "growth"
    ) {
        flags.push(("--entry", "unsigned decimal u32 entry ID local to snapshot; default root 0; growth uses AFTER scope"));
    }
    if matches!(
        command,
        "scan"
            | "children"
            | "largest"
            | "scans"
            | "compare"
            | "volumes"
            | "query"
            | "insights"
            | "growth"
            | "profile"
            | "coverage"
    ) {
        flags.push((
            "--limit",
            "integer 1..100; default 10; byte budget may shorten page",
        ));
    }
    if matches!(
        command,
        "children"
            | "largest"
            | "scans"
            | "compare"
            | "volumes"
            | "insights"
            | "growth"
            | "profile"
            | "coverage"
    ) {
        flags.push((
            "--cursor",
            "opaque next_cursor from matching query; default first page; reuse unchanged",
        ));
    }
    match command {
        "scan" => flags.extend([
            ("--save", "boolean flag, default false; retain metadata and permit three-run metadata pruning after publication"),
            ("--output", "absolute metadata directory alias; implies --save; last store/output wins"),
            ("--capture-apfs-clones", "boolean flag, default false; opt-in full DATA clone metadata within root; required for scan --metric apfs-data-clone"),
            ("--max-entries", "positive usize decimal; default unlimited (usize::MAX)"),
            ("--max-duration", "positive u64 decimal seconds; default unlimited (Duration::MAX)"),
            ("--memory-budget", "integer bytes 262144..268435456; default 234881024"),
            ("--metadata-budget", "u64 decimal bytes; default 4294967296 new-measurement allowance; explicit value limits old + new retained metadata; separate workspace/free-space guard; insufficient budget fails"),
            ("--workers", "integer 1 only; default 1; other values unsupported (exit 6)"),
            ("--progress", "off|text|ndjson; default off; stderr only")]),
        "children"|"largest" => { flags.extend([("--search", "UTF-8 name search; default empty"), ("--min-bytes", "u64 decimal bytes; default 0")]); if command == "children" { flags.push(("--kind", "file|directory|symlink|special; default all")); } },
        "profile" => flags.extend([
            ("--min-age-days", "u64 decimal days relative to saved end time; default no age filter; unknown/future mtime excluded when filtering"),
            ("--min-bytes", "u64 decimal bytes; default no minimum"),
            ("--extension", "suffix filter; ASCII lowercase normalization; mutually exclusive with --no-extension and --type-key"),
            ("--no-extension", "boolean flag; files without suffix; mutually exclusive with --extension and --type-key"),
            ("--type-key", "opaque saved type key; mutually exclusive with --extension and --no-extension"),
            ("--view", "files|types; default files"), ("--sort", "bytes|count; default bytes")]),
        "growth" => flags.extend([("--grouping", "area|category; default area"), ("--view", "groups|files; default groups"), ("--group", "stable group_key from previous response; default all"), ("--change", "all|grown|shrunk|added|removed|unknown; default all")]),
        "insights" => flags.push(("--category", "category from category rollup; default rollups; selecting category returns candidate scopes")),
        "coverage" => flags.push(("--reason", "saved coverage reason code; default all reasons")),
        "query" => flags.extend([("--sql", "required single read-only SELECT or WITH statement; no arbitrary file/network access or writes"), ("--before", "saved BEFORE snapshot ID; enables before_entries and changes against --scan AFTER"), ("--offset", "usize decimal; default 0; follow next_offset; ORDER BY for stable pages")]),
        "history-prune" => flags.extend([("--dry-run", "boolean flag; default; mutually exclusive with --apply"), ("--apply", "boolean flag; delete only validated old Macscan metadata, keep latest 3 runs; mutually exclusive with --dry-run")]),
        _ => {}
    }
    flags
}

// Recognize help only in option positions, respecting value arity and --.
pub fn help_requested(command: &str, args: &[std::ffi::OsString]) -> bool {
    let flags = options(command);
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            break;
        }
        if arg == "--help" || arg == "-h" {
            return true;
        }
        let takes_value = arg.to_str().is_some_and(|flag| {
            flags.iter().any(|(name, _)| *name == flag)
                && !matches!(
                    flag,
                    "--save" | "--capture-apfs-clones" | "--no-extension" | "--dry-run" | "--apply"
                )
        });
        index += if takes_value { 2 } else { 1 };
    }
    false
}

pub fn identity() -> Value {
    json!({"cli_version":env!("CARGO_PKG_VERSION"),"build_revision":env!("MACSCAN_BUILD_REVISION"),"engine_version":macscan_engine::ENGINE_VERSION,"data_schema_version":"2.0","native_engine_schema_version":macscan_engine::SCHEMA_VERSION})
}

pub fn help(command: &str) -> Option<Value> {
    if !COMMANDS.contains(&command) {
        return None;
    }
    let arguments = match command {
        "scan" => "exactly one ROOT path (relative to current working directory or absolute); use -- before a root starting with --",
        "compare"|"growth" => "exactly BEFORE_ID AFTER_ID: saved snapshot/volume-scan ID strings, never whole-Mac run IDs",
        _ => "no positional operands",
    };
    let effects = match command {
        "scan" => "reads metadata, no file contents; default private temporary store removed at command end, retained=false. --save/--output publishes retained=true metadata; successful publication may prune to latest 3 runs (grouped whole-Mac counts once). Never deletes scanned files. No atomic filesystem snapshot.",
        "history-prune" => "default dry-run reads only; --apply prunes validated Macscan metadata to latest 3 runs, protecting active jobs/groups. Never deletes scanned files.",
        _ => "read-only; no scan, metadata publication or history pruning; help creates no store and requires neither HOME nor GUI",
    };
    let mut value = json!({"schema_version":"3.0","outcome":"ok","identity":identity(),"command":command,"arguments":arguments,"help":"--help or -h; optionally --format; no other arguments; no scan/store access","options":options(command).into_iter().map(|(k,v)|(k.to_owned(),json!(v))).collect::<serde_json::Map<_,_>>(),"side_effects":effects,"output":{"format":"JSON object or NDJSON start/record/end; errors and optional progress on stderr","maximum_bytes":8192,"unknown":"null, never zero","partial":"usable incomplete coverage; no deletion evidence"},"exit_codes":{"ok":0,"partial":10,"cancelled":130,"input":2,"io":3,"not_found":4,"budget":5,"unsupported":6,"busy":7}});
    if command == "query" {
        value["views"] = json!({"entries":"selected snapshot, contextual category fields","before_entries":"requires --before","changes":"requires --before; saved BEFORE/AFTER path comparison","entries_saved":"raw saved metadata","entries_classified([ENTRY_ID])":"contextual classification for selected IDs"});
    }
    Some(value)
}

pub fn capabilities() -> Value {
    json!({"schema_version":"3.0","outcome":"ok","engine_version":macscan_engine::ENGINE_VERSION,"identity":identity(),"platform":"macos","backend":"darwin_descriptor_relative","commands":COMMANDS,"read_only_targets":true,"file_content_reads":false,"network":false,"cleanup":false,"scanned_files_cleanup":false,"metadata_retention":{"keep_runs":3,"trigger":"successful saved publication or explicit history-prune --apply","scope":"validated Macscan metadata only; grouped runs count once"},"storage":"parquet_zstd","query_engine":macscan_analysis::Store::version(),"supported_metrics":["logical","allocated","apfs-data-clone"],"private_bytes":null,"clone_metadata":"opt_in_streaming_parquet","apfs_data_clone_algorithm":"apfs-data-clone-1","apfs_data_clone_capture_version":1,"apfs_data_clone_limitations":"full DATA only, within measured root; partial and non-DATA sharing not deduplicated; not unique/private/reclaimable space","atomic_filesystem_snapshot":false,"workers":1,"agent_response_budget_bytes":8192})
}

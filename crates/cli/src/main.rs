mod discovery;
use macscan_analysis::{Store, DEFAULT_LIMIT, OUTPUT_BUDGET, FULL_HISTORY_BUDGET};
use macscan_engine::{
    volumes, CompareQuery, Metric, Query, QueryKind, ScanOptions,
    ENGINE_VERSION,
};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static CANCEL: AtomicBool = AtomicBool::new(false);
extern "C" fn cancel(_: libc::c_int) {
    CANCEL.store(true, Ordering::Relaxed);
}

struct Error {
    exit: u8,
    code: &'static str,
    operation: &'static str,
    detail: String,
}
type Result<T> = std::result::Result<T, Error>;
impl Error {
    fn usage(detail: &str) -> Self {
        Self {
            exit: 2,
            code: "INVALID_ARGUMENT",
            operation: "arguments",
            detail: detail.into(),
        }
    }
    fn io(operation: &'static str, e: io::Error) -> Self {
        let (exit, code) = match e.kind() {
            io::ErrorKind::NotFound => (4, "NOT_FOUND"),
            io::ErrorKind::InvalidInput => (2, "INVALID_INPUT"),
            io::ErrorKind::Unsupported => (6, "UNSUPPORTED"),
            io::ErrorKind::AlreadyExists => (3, "ALREADY_EXISTS"),
            io::ErrorKind::BrokenPipe => (3, "BROKEN_PIPE"),
            io::ErrorKind::StorageFull => (5, "METADATA_LIMIT"),
            io::ErrorKind::Interrupted => (130, "CANCELLED"),
            io::ErrorKind::WouldBlock if matches!(operation, "run_lock" | "history-prune") => (7, "HISTORY_BUSY"),
            _ => (3, "IO_ERROR"),
        };
        Self {
            exit,
            code,
            operation,
            detail: e.to_string(),
        }
    }
}
struct Options {
    operands: Vec<OsString>,
    query: Query,
    scan: ScanOptions,
    scan_id: Option<String>,
    reason: Option<String>,
    store: PathBuf,
    save: bool,
    ndjson: bool,
    progress: String,
    metadata_budget: u64,
    explicit_budget: bool,
    prune_apply: bool,
    prune_mode: bool,
    sql: Option<String>,
    before: Option<String>,
    offset: usize,
    category: Option<String>,
    growth_grouping: macscan_analysis::GrowthGrouping,
    growth_files: bool,
    growth_group: Option<String>,
    growth_change: String,
    profile: macscan_analysis::FileProfileQuery,
}
fn utf8(value: &OsString) -> Result<&str> {
    value
        .to_str()
        .ok_or_else(|| Error::usage("option must be UTF-8"))
}
fn unsigned(value: &OsString) -> Result<u64> {
    let text = utf8(value)?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::usage("expected unsigned decimal integer"));
    }
    text.parse().map_err(|_| Error::usage("integer overflow"))
}
fn parse(command: &str, args: &[OsString]) -> Result<Options> {
    let home = std::env::var_os("HOME").ok_or_else(|| Error::usage("HOME unavailable"))?;
    let mut options = Options {
        operands: Vec::new(),
        query: Query {
            limit: DEFAULT_LIMIT,
            ..Query::default()
        },
        scan: ScanOptions::full(),
        scan_id: None,
        reason: None,
        store: PathBuf::from(home).join("Library/Application Support/Macscan"),
        save: false,
        ndjson: false,
        progress: "off".into(),
        metadata_budget: FULL_HISTORY_BUDGET,
        explicit_budget: false,
        prune_apply: false,
        prune_mode: false,
        sql: None,
        before: None,
        offset: 0,
        category: None,
        growth_grouping: macscan_analysis::GrowthGrouping::Area,
        growth_files: false,
        growth_group: None,
        growth_change: "all".into(),
        profile: Default::default(),
    };
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            options.operands.extend_from_slice(&args[index + 1..]);
            break;
        }
        if !arg.as_encoded_bytes().starts_with(b"--") {
            options.operands.push(arg.clone());
            index += 1;
            continue;
        }
        let flag = utf8(arg)?;
        if !discovery::options(command).iter().any(|(name,_)| *name == flag) {
            return Err(Error::usage("unknown flag or flag is not supported by this command"));
        }
        if flag == "--capture-apfs-clones" {
            options.scan.capture_clones=true;index+=1;continue;
        }
        if matches!(flag,"--dry-run"|"--apply") {
            if options.prune_mode {return Err(Error::usage("choose either --dry-run or --apply"));}
            options.prune_mode=true;options.prune_apply=flag=="--apply";index+=1;continue;
        }
        if flag == "--save" {
            options.save = true;
            index += 1;
            continue;
        }
        if flag == "--no-extension" {
            if options.profile.type_key.is_some() { return Err(Error::usage("choose one extension/type filter")); }
            options.profile.type_key = Some("none".into());
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| Error::usage("flag requires a value"))?;
        match flag {
            "--format" => match utf8(value)? {
                "json" => options.ndjson = false,
                "ndjson" => options.ndjson = true,
                _ => return Err(Error::usage("format must be json or ndjson")),
            },
            "--store" =>
            {
                options.store = PathBuf::from(value)
            }
            "--output" => {
                options.save = true;
                options.store = PathBuf::from(value);
            }
            "--scan" =>
            {
                options.scan_id = Some(utf8(value)?.into())
            }
            "--reason" => options.reason=Some(utf8(value)?.into()),
            "--metric" =>
            {
                options.query.metric =
                    Metric::parse(utf8(value)?).map_err(|e| Error::io("metric", e))?
            }
            "--entry" => {
                options.query.entry = u32::try_from(unsigned(value)?)
                    .map_err(|_| Error::usage("entry ID overflow"))?
            }
            "--limit" =>
            {
                options.query.limit =
                    usize::try_from(unsigned(value)?).map_err(|_| Error::usage("limit overflow"))?
            }
            "--cursor" =>
            {
                options.query.cursor = Some(utf8(value)?.into())
            }
            "--min-age-days" => options.profile.min_age_days = Some(unsigned(value)?),
            "--min-bytes" if command == "profile" => options.profile.min_bytes = Some(unsigned(value)?),
            "--extension" | "--type-key" => {
                if options.profile.type_key.is_some() { return Err(Error::usage("choose one extension/type filter")); }
                options.profile.type_key = Some(if flag == "--extension" {
                    macscan_analysis::FileProfileQuery::extension_key(utf8(value)?).map_err(|e|Error::io("extension",e))?
                } else { utf8(value)?.into() });
            },
            "--view" if command == "profile" => options.profile.types = match utf8(value)? {
                "files" => false, "types" => true, _ => return Err(Error::usage("view must be files or types")),
            },
            "--sort" => options.profile.sort_count = match utf8(value)? {
                "bytes" => false, "count" => true, _ => return Err(Error::usage("sort must be bytes or count")),
            },
            "--grouping" => options.growth_grouping = match utf8(value)? {
                "area" => macscan_analysis::GrowthGrouping::Area,
                "category" => macscan_analysis::GrowthGrouping::Category,
                _ => return Err(Error::usage("grouping must be area or category")),
            },
            "--view" => options.growth_files = match utf8(value)? {
                "groups" => false, "files" => true,
                _ => return Err(Error::usage("view must be groups or files")),
            },
            "--group" => options.growth_group = Some(utf8(value)?.into()),
            "--change" => options.growth_change = utf8(value)?.into(),
            "--category" => options.category = Some(utf8(value)?.into()),
            "--sql" => options.sql = Some(utf8(value)?.into()),
            "--before" => options.before = Some(utf8(value)?.into()),
            "--offset" => {
                options.offset = usize::try_from(unsigned(value)?)
                    .map_err(|_| Error::usage("offset overflow"))?
            }
            "--search" => {
                options.query.search = utf8(value)?.into()
            }
            "--min-bytes" => {
                options.query.min_bytes = unsigned(value)?
            }
            "--kind" => {
                options.query.only_kind = Some(match utf8(value)? {
                    "file" => macscan_engine::EntryKind::File,
                    "directory" => macscan_engine::EntryKind::Directory,
                    "symlink" => macscan_engine::EntryKind::Symlink,
                    "special" => macscan_engine::EntryKind::Special,
                    _ => return Err(Error::usage("unknown kind")),
                })
            }
            "--max-entries" => {
                options.scan.max_entries = usize::try_from(unsigned(value)?)
                    .map_err(|_| Error::usage("entry count overflow"))?
            }
            "--max-duration" => {
                options.scan.max_duration = Duration::from_secs(unsigned(value)?)
            }
            "--memory-budget" => {
                options.scan.max_memory_bytes = usize::try_from(unsigned(value)?)
                    .map_err(|_| Error::usage("memory budget overflow"))?
            }
            "--metadata-budget" => {options.metadata_budget = unsigned(value)?;options.explicit_budget=true;},
            "--workers" => {
                if unsigned(value)? != 1 {
                    return Err(Error {
                        exit: 6,
                        code: "UNSUPPORTED_WORKERS",
                        operation: "arguments",
                        detail: "currently supports one bounded worker".into(),
                    });
                }
            }
            "--progress" => {
                let mode = utf8(value)?;
                if !matches!(mode, "off" | "text" | "ndjson") {
                    return Err(Error::usage("progress must be off, text or ndjson"));
                }
                options.progress = mode.into();
            }
            _ => {
                return Err(Error::usage(
                    "unknown flag or flag is not supported by this command",
                ))
            }
        }
        index += 2;
    }
    if options.query.limit == 0 || options.query.limit > 100 {
        return Err(Error::usage("limit must be 1..100"));
    }
    if options.scan.max_entries == 0
        || options.scan.max_duration.is_zero()
        || options.scan.max_memory_bytes < 256 * 1024
        || options.scan.max_memory_bytes > 256 * 1024 * 1024
    {
        return Err(Error::usage("scan limits out of supported range"));
    }
    if !options.store.is_absolute() {
        return Err(Error::usage(
            "store/output must be an absolute directory path",
        ));
    }
    if options.store.components().any(|c| {
        matches!(
            c,
            std::path::Component::CurDir | std::path::Component::ParentDir
        )
    }) {
        return Err(Error::usage(
            "store/output cannot contain dot or parent components",
        ));
    }
    if command=="scan" && options.query.metric==Metric::ApfsDataClone && !options.scan.capture_clones {
        return Err(Error::usage("scan --metric apfs-data-clone requires explicit --capture-apfs-clones"));
    }
    if options.query.metric == Metric::Private {
        return Err(Error {
            exit: 6,
            code: "UNSUPPORTED_METRIC",
            operation: "metric",
            detail: "private bytes have not been validated on this backend".into(),
        });
    }
    Ok(options)
}
fn schema() -> Value {
    json!({"schema_version":"3.0","engine_version":ENGINE_VERSION,"status":"ok",
        "identity":discovery::identity(),"commands":discovery::COMMANDS,"command_help":"schema --command NAME or NAME --help",
        "storage":"ZSTD compressed Parquet, explicit --save; small manifest.json; no persistent DuckDB database",
        "apfs_data_clone_contract":{"algorithm_version":"apfs-data-clone-1","capture_version":1,"label":"Begrænset DATA-klonkorrigeret allokering","opt_in":"scan --capture-apfs-clones","limitations":"Within measurement root; full DATA only, partial and non-DATA retained; global raw-path owners; not unique/private/reclaimable space","documentation":"docs/apfs-data-clone.md"},
        "profile_contract":{"version":"file-profile-1","view":["files","types"],"sort":["bytes","count"],"scope":"saved --entry with raw path component boundaries","age":"saved ended_unix_ns; inclusive integer threshold; unknown/future mtime excluded","type":"raw final suffix hint; ASCII lowercase; opaque type_key preserves non-UTF8 identity","documentation":"docs/file-profile.md"},
        "growth_contract":{"version":"growth-1","rules":"insights-1","grouping":["area","category"],"view":["groups","files"],"change":["all","grown","shrunk","added","removed","unknown"],"group":"stable group_key from a previous response","scope":"AFTER --entry; BEFORE matched by saved raw path key","accounting":"changed regular-file contributions, no directory subtree overlap","documentation":"docs/growth.md"},
        "insights_contract":{"version":"insights-1","default":"category rollups","category":"paginated contributing candidate scopes","counts":"regular file paths including hardlinks, not physical identities","rules":"docs/insights.md"},
        "category_contract":{"version":"context-1","category":"derived from saved raw metadata; probable contextual rules or unverified name hints, never deletion approval","saved_category":"legacy snapshot value, not evidence","coverage":"complete, partial, or context_unavailable; applies to saved context, not live validity","rules":"docs/category-rules.md"},
        "query_views":{"entries":"selected scan; schema below","changes":"available with --before ID; joins selected AFTER and BEFORE scan","before_entries":"available with --before ID"},
        "columns":["entry_id","parent_id","name_display","path_display","name_bytes","path_bytes","path_key","kind","logical_bytes","allocated_bytes","accounted_allocated_bytes","own_allocated_bytes","observed_logical_bytes","observed_allocated_bytes","observed_accounted_allocated_bytes","device_id","inode","link_count","observed_link_count","charged_to","mtime_unix_seconds","coverage_complete","enumeration_complete","category","saved_category","category_rule_id","category_rule_version","category_reason","category_coverage","category_evidence_source","private_bytes","clone_status","clone_id","clone_extended_flags","clone_data_allocated_bytes","clone_device_id","apfs_data_clone_bytes","observed_apfs_data_clone_bytes","own_apfs_data_clone_bytes","apfs_data_clone_owner","apfs_data_clone_reason"],
        "options":{"store":"absolute metadata directory","limit":"1..100, default 10; responses always <=8192 bytes","offset":"SQL pagination; use next_offset, use ORDER BY for stable SQL pages","cursor":"predefined query pagination; use next_cursor unchanged","metric":["logical","allocated","apfs-data-clone"],"progress":["off","text","ndjson"],"format":["json","ndjson"]},
        "sql_examples":["SELECT path_display,accounted_allocated_bytes FROM entries WHERE kind='file' ORDER BY accounted_allocated_bytes DESC LIMIT 10","SELECT category,count(*) AS entries,sum(accounted_allocated_bytes) AS bytes FROM entries WHERE kind='file' GROUP BY category ORDER BY bytes DESC","SELECT path_display,logical_delta FROM changes WHERE logical_delta>0 ORDER BY logical_delta DESC LIMIT 10"],
        "result_contract":{"schema_version":"3.0","outcome":"ok|partial|failed|cancelled; independent of saved status","exit_codes":{"ok":0,"partial":10,"cancelled":130,"input":2,"io":3,"not_found":4,"budget":5,"unsupported":6,"busy":7},"documentation":"docs/cli-contract.md","all_SQL_numbers":"exact decimal strings; booleans and null preserved","null":"unknown, never zero","allocated":"scoped hardlink-accounted OS allocation, not unique APFS or reclaimable bytes","partial":"unknown areas are not deletion evidence","output_budget_bytes":OUTPUT_BUDGET},
        "limits":{"metadata_budget_bytes":FULL_HISTORY_BUDGET,"max_entries":null,"max_duration_seconds":null,"scan_mode":"streaming_full","scanner_workers":1,"scanned_files_cleanup":false,"metadata_retention":{"keep_runs":3,"deletion_scope":"saved Macscan metadata only; scanned files are never deleted","trigger":"after successful publication; whole-Mac counts once"},"metadata_budget_scope":"default new measurement allowance; explicit --metadata-budget combined total","UI":"Tauri sunburst"}})
}
#[derive(Default)]
struct Publication {
    scan_id: Option<String>,
    retained: bool,
    retention_warning: Option<String>,
}
fn usable(complete: bool) -> u8 {
    if complete { 0 } else { 10 }
}
fn run(args: &[OsString], publication: &mut Publication) -> Result<u8> {
    let command = args.first().map(utf8).transpose()?.unwrap_or("--help");
    let command = match command { "--help"|"-h" => "schema", other => other };
    if !discovery::COMMANDS.contains(&command) { return Err(Error::usage("unknown command; use schema")); }
    let tail = args.get(1..).unwrap_or_default();
    if discovery::help_requested(command, tail) {
        let mut ndjson = false;
        let mut index = 0;
        while index < tail.len() {
            match utf8(&tail[index])? {
                "--help"|"-h" => index += 1,
                "--format" => {
                    let format = tail.get(index+1).ok_or_else(||Error::usage("flag requires a value"))?;
                    ndjson = match utf8(format)? {"json"=>false,"ndjson"=>true,_=>return Err(Error::usage("format must be json or ndjson"))};
                    index += 2;
                }
                _ => return Err(Error::usage("command help accepts only --help/-h and --format")),
            }
        }
        emit(discovery::help(command).unwrap(), ndjson).map_err(|e| Error::io("stdout",e))?;
        return Ok(0);
    }
    if matches!(command,"schema"|"capabilities"|"--version") {
        let mut selected = None;
        let mut ndjson = false;
        let mut index = 0;
        while index < tail.len() {
            let flag = utf8(&tail[index])?;
            let value = tail.get(index+1).ok_or_else(||Error::usage("flag requires a value"))?;
            match flag {
                "--command" if command == "schema" => selected = Some(utf8(value)?),
                "--format" => ndjson = match utf8(value)? {"json"=>false,"ndjson"=>true,_=>return Err(Error::usage("format must be json or ndjson"))},
                _ => return Err(Error::usage("unknown flag or unexpected operand")),
            }
            index += 2;
        }
        let mut result = match command {
            "schema" => match selected {Some(name)=>discovery::help(name).ok_or_else(||Error::usage("unknown command; use schema"))?,None=>schema()},
            "capabilities" => discovery::capabilities(),
            _ => json!({"identity":discovery::identity(),"engine_version":ENGINE_VERSION,"duckdb_version":Store::version()}),
        };
        result["schema_version"] = json!("3.0"); result["outcome"] = json!("ok");
        emit(result, ndjson).map_err(|e|Error::io("stdout",e))?;
        return Ok(0);
    }
    let options = parse(command, tail)?;
    // Only this process's scan/query is interrupted; no existing user process.
    unsafe {
        libc::signal(libc::SIGINT, cancel as *const () as libc::sighandler_t);
    }
    let mut code = 0;
    let mut result = match command {
        "volumes" => volumes(options.query.limit, options.query.cursor.as_deref())
            .map_err(|e| Error::io("volumes", e))?,
        "history-prune" => {
            if !options.operands.is_empty(){return Err(Error::usage("history-prune takes no operands"));}
            Store::open(&options.store,false).and_then(|s|s.prune_history(options.prune_apply)).map_err(|e|Error::io("history-prune",e))?
        }
        "scan" => {
            if options.operands.len() != 1 {
                return Err(Error::usage("scan requires exactly one explicit ROOT"));
            }
            let temporary=if options.save {None}else{Some(TemporaryStore::new().map_err(|e|Error::io("temporary_store",e))?)};
            let path=temporary.as_ref().map(|t|t.0.as_path()).unwrap_or(&options.store);
            let store=Store::open(path,true).map_err(|e|Error::io("open_store",e))?;
            let store=store.begin_run().map_err(|e|Error::io("run_lock",e))?;
            let budget=if options.explicit_budget{options.metadata_budget}else{store.measurement_budget(options.metadata_budget).map_err(|e|Error::io("metadata_allowance",e))?};
            let id=store.scan_to_store(&PathBuf::from(&options.operands[0]),&options.scan,budget,&CANCEL,|p|{
                match options.progress.as_str() {
                    "ndjson"=>{let _=writeln!(io::stderr(),"{}",json!({"schema_version":"3.0","type":"progress","entries_seen":p.entries_seen,"elapsed_ms":p.elapsed_ms,"finished":p.finished}));},
                    "text"=>{let _=writeln!(io::stderr(),"{} entries, {} ms{}",p.entries_seen,p.elapsed_ms,if p.finished {"; scan finished"}else{""});},_=>{}
                }
            }).map_err(|e|Error::io("scan",e))?;
            publication.scan_id = Some(id.clone());
            publication.retained = options.save;
            if CANCEL.load(Ordering::Relaxed) {
                return Err(Error::io("scan", io::Error::new(io::ErrorKind::Interrupted, "cancelled")));
            }
            // Successful publication, not report delivery, triggers existing retention.
            if options.save { publication.retention_warning = store.retention_warning(&id); }
            let mut result=store.report(&id,&options.query,&CANCEL).map_err(|e|Error::io("query",e))?;
            code = usable(result["selected_coverage"]["complete"] == true);
            if !options.save {
                result["next_cursor"]=Value::Null;
                result["navigation_note"]=json!("Use --save for retained Parquet history, queries and comparison.");
            }
            if let Some(w) = &publication.retention_warning { result["retention_warning"] = json!(w); }
            result["command"] = json!("scan");
            result["retained"] = json!(options.save);
            result
        }
        "profile" => {
            if !options.operands.is_empty() { return Err(Error::usage("unexpected operands")); }
            let id = options.scan_id.as_deref().ok_or_else(|| Error::usage("--scan ID is required"))?;
            let store = Store::open(&options.store,false).map_err(|e|Error::io("open_store",e))?;
            let mut q = options.profile;
            q.entry = options.query.entry; q.metric = options.query.metric;
            q.limit = options.query.limit; q.cursor = options.query.cursor;
            let result = store.file_profile(id,&q,&CANCEL).map_err(|e|Error::io("profile",e))?;
            code = usable(result["coverage"]["complete"] == true);
            result
        }
        "growth" => {
            if options.operands.len() != 2 { return Err(Error::usage("growth requires BEFORE_ID AFTER_ID")); }
            let store = Store::open(&options.store, false).map_err(|e| Error::io("open_store", e))?;
            let result = store.growth(utf8(&options.operands[0])?, utf8(&options.operands[1])?, &macscan_analysis::GrowthQuery {
                entry: options.query.entry, metric: options.query.metric,
                grouping: options.growth_grouping, files: options.growth_files,
                group: options.growth_group, change: options.growth_change,
                limit: options.query.limit, cursor: options.query.cursor,
            }, &CANCEL).map_err(|e| Error::io("growth", e))?;
            code = usable(result["coverage"]["complete"] == true);
            result
        }
        "insights" => {
            if !options.operands.is_empty() {
                return Err(Error::usage("unexpected operands"));
            }
            let id = options.scan_id.as_deref()
                .ok_or_else(|| Error::usage("--scan ID is required"))?;
            let store = Store::open(&options.store, false)
                .map_err(|e| Error::io("open_store", e))?;
            let result = store.insights(id, &macscan_analysis::InsightsQuery {
                entry: options.query.entry,
                metric: options.query.metric,
                category: options.category,
                limit: options.query.limit,
                cursor: options.query.cursor,
            }, &CANCEL).map_err(|e| Error::io("insights", e))?;
            code = usable(result["coverage"]["complete"] == true);
            result
        }
        "query" => {
            if !options.operands.is_empty() {
                return Err(Error::usage("query takes --scan ID --sql SELECT"));
            }
            let id = options
                .scan_id
                .as_deref()
                .ok_or_else(|| Error::usage("--scan ID is required"))?;
            let sql = options
                .sql
                .as_deref()
                .ok_or_else(|| Error::usage("--sql SELECT is required"))?;
            let store =
                Store::open(&options.store, false).map_err(|e| Error::io("open_store", e))?;
            let result = store
                .query_sql(
                    id,
                    options.before.as_deref(),
                    sql,
                    options.query.limit,
                    options.offset,
                    &CANCEL,
                )
                .map_err(|e| Error::io("query", e))?;
            // SQL may select arbitrary columns: use the whole participating measurements.
            code = usable(result["coverage"]["complete"] == true);
            result
        }
        "compare" => {
            if options.operands.len() != 2 {
                return Err(Error::usage("compare requires BEFORE_ID AFTER_ID"));
            }
            let store =
                Store::open(&options.store, false).map_err(|e| Error::io("open_store", e))?;
            let result = store
                .compare(
                    utf8(&options.operands[0])?,
                    utf8(&options.operands[1])?,
                    &CompareQuery {
                        metric: options.query.metric,
                        limit: options.query.limit,
                        cursor: options.query.cursor,
                    },
                    &CANCEL,
                )
                .map_err(|e| Error::io("compare", e))?;
            code = usable(result["coverage"]["complete"] == true);
            result
        }
        "coverage" => {
            if !options.operands.is_empty() {return Err(Error::usage("unexpected operands"));}
            let id=options.scan_id.as_deref().ok_or_else(||Error::usage("--scan ID is required"))?;
            let store=Store::open(&options.store,false).map_err(|e|Error::io("open_store",e))?;
            let result = store.coverage(id,options.reason.as_deref(),options.query.cursor.as_deref(),options.query.limit,&CANCEL).map_err(|e|Error::io("coverage",e))?;
            // A delivered diagnostic report does not establish complete disk coverage.
            code = usable(result["status"] == "complete");
            result
        }
        "summary" | "children" | "largest" | "inspect" => {
            if !options.operands.is_empty() {
                return Err(Error::usage("unexpected operands"));
            }
            let id = options
                .scan_id
                .as_deref()
                .ok_or_else(|| Error::usage("--scan ID is required"))?;
            let store =
                Store::open(&options.store, false).map_err(|e| Error::io("open_store", e))?;
            let mut query = options.query;
            query.kind = match command {
                "summary" => QueryKind::Summary,
                "children" => QueryKind::Children,
                "largest" => QueryKind::Largest,
                _ => QueryKind::Inspect,
            };
            let result = store
                .report(id, &query, &CANCEL)
                .map_err(|e| Error::io("query", e))?;
            code = usable(result["selected_coverage"]["complete"] == true);
            result
        }
        "scans" => {
            let store =
                Store::open(&options.store, false).map_err(|e| Error::io("open_store", e))?;
            store
                .scans(options.query.limit, options.query.cursor.as_deref())
                .map_err(|e| Error::io("scans", e))?
        }
        _ => return Err(Error::usage("unknown command; use schema")),
    };
    if matches!(
        command,
        "--help" | "-h" | "schema" | "--version" | "capabilities" | "volumes" | "scans"
    ) && !options.operands.is_empty()
    {
        return Err(Error::usage("unexpected operands"));
    }
    if CANCEL.load(Ordering::Relaxed) {
        return Err(Error::io("response", io::Error::new(io::ErrorKind::Interrupted, "cancelled")));
    }
    // CLI envelopes evolve independently of saved/native analysis formats.
    result["schema_version"] = json!("3.0");
    result["outcome"] = json!(if code == 10 { "partial" } else { "ok" });
    // Preserve the original scoped pagination if added CLI metadata needs space.
    let binding = if command == "scan" {
        publication.scan_id.as_ref().map(|id| format!("p1:{id}:children:0:{}::all:0", result["metric"].as_str().unwrap_or("allocated")))
    } else if !matches!(command, "scans" | "volumes") {
        result["next_cursor"].as_str().and_then(|cursor| cursor.rsplit_once(':').map(|(prefix, _)| prefix.to_owned()))
    } else { None };
    let mut result = macscan_analysis::compact_response(result, binding.as_deref())
        .map_err(|e| Error::io("response", e))?;
    if command == "scan" && !options.save { result["next_cursor"] = Value::Null; }
    emit(result, options.ndjson).map_err(|e| Error::io("stdout", e))?;
    Ok(code)
}
struct TemporaryStore(PathBuf);
impl TemporaryStore {
    fn new() -> io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let nanos=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        let path=std::env::temp_dir().canonicalize()?.join(format!("macscan-preview-{}-{nanos}",std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for TemporaryStore {
    fn drop(&mut self) { let _=std::fs::remove_dir_all(&self.0); }
}
fn emit(result: Value, ndjson: bool) -> io::Result<()> {
    let bytes = macscan_analysis::encode_response(result, ndjson)?;
    if bytes.len() > OUTPUT_BUDGET {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "response exceeds 8 KiB",
        ));
    }
    let mut out = io::stdout().lock();
    out.write_all(&bytes)?;
    out.flush()
}
// Diagnostic text is bounded explicitly; exact result values are never rounded.
fn bounded_detail(text: &str, maximum: usize) -> (&str, bool) {
    let mut end = text.len().min(maximum);
    while !text.is_char_boundary(end) { end -= 1; }
    (&text[..end], end < text.len())
}
fn error_response(e: &Error, publication: &Publication, ndjson: bool) -> Value {
    let (detail, truncated) = bounded_detail(&e.detail, 768);
    let hint = match e.exit {
        2 => Some("Check the command and flags against macscan schema."),
        4 => Some("Check that the requested root, store, scan or entry exists."),
        5 => Some("Review the metadata/workspace budget and available disk space before trying again."),
        7 => Some("Wait for the active scan or history cleanup to finish, then retry."),
        _ => None,
    };
    let mut value = json!({"schema_version":"3.0","engine_version":ENGINE_VERSION,
        "status":"failed","outcome":if e.exit == 130 {"cancelled"}else{"failed"},
        "error":{"code":e.code,"operation":e.operation,"detail":detail,
            "detail_truncated":truncated,"retryable":e.exit == 7}});
    if let Some(hint) = hint { value["error"]["hint"] = json!(hint); }
    if let Some(id) = &publication.scan_id {
        value["scan_id"] = json!(id);
        value["retained"] = json!(publication.retained);
        if let Some(warning) = &publication.retention_warning {
            let (warning, truncated) = bounded_detail(warning, 256);
            value["retention_warning"] = json!(warning);
            value["retention_warning_truncated"] = json!(truncated);
        }
    }
    if ndjson { value["type"] = json!("error"); }
    value
}
fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    // Select error framing even when argument validation itself fails.
    let ndjson = args.split(|arg| arg == "--").next().unwrap_or_default()
        .windows(2).filter(|pair| pair[0] == "--format")
        .last().is_some_and(|pair| pair[1] == "ndjson");
    let mut publication = Publication::default();
    let code = match run(&args, &mut publication) {
        Ok(code) => code,
        Err(e) => {
            let error = error_response(&e, &publication, ndjson);
            let _ = writeln!(io::stderr(), "{error}");
            e.exit
        }
    };
    std::process::ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escaped_error_diagnostics_fit_the_serialized_budget() {
        let error = Error::io("stdout", io::Error::other("\u{0001}".repeat(10000)));
        let publication = Publication { scan_id: Some("12345678901234567890-12345-1".into()),
            retained: true, retention_warning: Some("\u{0002}".repeat(10000)) };
        for ndjson in [false, true] {
            let value = error_response(&error, &publication, ndjson);
            assert!(serde_json::to_vec(&value).unwrap().len() + 1 <= OUTPUT_BUDGET);
            assert_eq!(value["error"]["retryable"], false);
            assert_eq!(value["error"]["detail_truncated"], true);
            assert_eq!(value["retention_warning_truncated"], true);
        }
    }
}

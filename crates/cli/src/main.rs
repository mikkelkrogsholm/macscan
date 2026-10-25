use macscan_analysis::{Store, DEFAULT_LIMIT, OUTPUT_BUDGET};
use macscan_engine::{
    capabilities, scan_with_progress, volumes, CompareQuery, Metric, Query, QueryKind, ScanOptions,
    ENGINE_VERSION, STORE_BUDGET,
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
    store: PathBuf,
    save: bool,
    ndjson: bool,
    progress: String,
    metadata_budget: u64,
    sql: Option<String>,
    before: Option<String>,
    offset: usize,
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
        scan: ScanOptions::default(),
        scan_id: None,
        store: PathBuf::from(home).join("Library/Application Support/Macscan"),
        save: false,
        ndjson: false,
        progress: "off".into(),
        metadata_budget: STORE_BUDGET,
        sql: None,
        before: None,
        offset: 0,
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
        if flag == "--save" {
            if command != "scan" {
                return Err(Error::usage("--save is only valid for scan"));
            }
            options.save = true;
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
            "--store"
                if matches!(
                    command,
                    "scan"
                        | "summary"
                        | "children"
                        | "largest"
                        | "inspect"
                        | "scans"
                        | "compare"
                        | "query"
                ) =>
            {
                options.store = PathBuf::from(value)
            }
            "--output" if command == "scan" => {
                options.save = true;
                options.store = PathBuf::from(value);
            }
            "--scan"
                if matches!(
                    command,
                    "summary" | "children" | "largest" | "inspect" | "query"
                ) =>
            {
                options.scan_id = Some(utf8(value)?.into())
            }
            "--metric"
                if matches!(
                    command,
                    "scan" | "summary" | "children" | "largest" | "inspect" | "compare"
                ) =>
            {
                options.query.metric =
                    Metric::parse(utf8(value)?).map_err(|e| Error::io("metric", e))?
            }
            "--entry" if matches!(command, "summary" | "children" | "largest" | "inspect") => {
                options.query.entry = u32::try_from(unsigned(value)?)
                    .map_err(|_| Error::usage("entry ID overflow"))?
            }
            "--limit"
                if matches!(
                    command,
                    "scan" | "children" | "largest" | "scans" | "compare" | "volumes" | "query"
                ) =>
            {
                options.query.limit =
                    usize::try_from(unsigned(value)?).map_err(|_| Error::usage("limit overflow"))?
            }
            "--cursor"
                if matches!(
                    command,
                    "children" | "largest" | "scans" | "compare" | "volumes"
                ) =>
            {
                options.query.cursor = Some(utf8(value)?.into())
            }
            "--sql" if command == "query" => options.sql = Some(utf8(value)?.into()),
            "--before" if command == "query" => options.before = Some(utf8(value)?.into()),
            "--offset" if command == "query" => {
                options.offset = usize::try_from(unsigned(value)?)
                    .map_err(|_| Error::usage("offset overflow"))?
            }
            "--search" if matches!(command, "children" | "largest") => {
                options.query.search = utf8(value)?.into()
            }
            "--min-bytes" if matches!(command, "children" | "largest") => {
                options.query.min_bytes = unsigned(value)?
            }
            "--kind" if command == "children" => {
                options.query.only_kind = Some(match utf8(value)? {
                    "file" => macscan_engine::EntryKind::File,
                    "directory" => macscan_engine::EntryKind::Directory,
                    "symlink" => macscan_engine::EntryKind::Symlink,
                    "special" => macscan_engine::EntryKind::Special,
                    _ => return Err(Error::usage("unknown kind")),
                })
            }
            "--max-entries" if command == "scan" => {
                options.scan.max_entries = usize::try_from(unsigned(value)?)
                    .map_err(|_| Error::usage("entry count overflow"))?
            }
            "--max-duration" if command == "scan" => {
                options.scan.max_duration = Duration::from_secs(unsigned(value)?)
            }
            "--memory-budget" if command == "scan" => {
                options.scan.max_memory_bytes = usize::try_from(unsigned(value)?)
                    .map_err(|_| Error::usage("memory budget overflow"))?
            }
            "--metadata-budget" if command == "scan" => options.metadata_budget = unsigned(value)?,
            "--workers" if command == "scan" => {
                if unsigned(value)? != 1 {
                    return Err(Error {
                        exit: 6,
                        code: "UNSUPPORTED_WORKERS",
                        operation: "arguments",
                        detail: "prototype currently supports one bounded worker".into(),
                    });
                }
            }
            "--progress" if command == "scan" => {
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
        || options.scan.max_entries > 1_000_000
        || options.scan.max_duration.is_zero()
        || options.scan.max_duration > Duration::from_secs(86_400)
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
    json!({"schema_version":"2.0","engine_version":ENGINE_VERSION,"status":"ok",
        "commands":["capabilities","volumes","scan ROOT [--save]","scans","summary --scan ID","children --scan ID --entry ID","largest --scan ID","inspect --scan ID --entry ID","query --scan ID --sql SELECT [--before ID]","compare BEFORE_ID AFTER_ID"],
        "storage":"ZSTD compressed Parquet, explicit --save; small manifest.json; no persistent DuckDB database",
        "query_views":{"entries":"selected scan; schema below","changes":"available with --before ID; joins selected AFTER and BEFORE scan","before_entries":"available with --before ID"},
        "columns":["entry_id","parent_id","name_display","path_display","name_bytes","path_bytes","path_key","kind","logical_bytes","allocated_bytes","accounted_allocated_bytes","own_allocated_bytes","observed_logical_bytes","observed_allocated_bytes","observed_accounted_allocated_bytes","device_id","inode","link_count","observed_link_count","charged_to","mtime_unix_seconds","coverage_complete","enumeration_complete","category","private_bytes"],
        "options":{"store":"absolute metadata directory","limit":"1..100, default 10; responses always <=8192 bytes","offset":"SQL pagination; use next_offset, use ORDER BY for stable SQL pages","cursor":"predefined query pagination; use next_cursor unchanged","metric":["logical","allocated"],"progress":["off","text","ndjson"],"format":["json","ndjson"]},
        "sql_examples":["SELECT path_display,accounted_allocated_bytes FROM entries WHERE kind='file' ORDER BY accounted_allocated_bytes DESC LIMIT 10","SELECT category,count(*) AS entries,sum(accounted_allocated_bytes) AS bytes FROM entries WHERE kind='file' GROUP BY category ORDER BY bytes DESC","SELECT path_display,logical_delta FROM changes WHERE logical_delta>0 ORDER BY logical_delta DESC LIMIT 10"],
        "result_contract":{"schema_version":"2.0","all_SQL_numbers":"exact decimal strings; booleans and null preserved","null":"unknown, never zero","allocated":"scoped hardlink-accounted OS allocation, not unique APFS or reclaimable bytes","partial":"unknown areas are not deletion evidence","output_budget_bytes":OUTPUT_BUDGET},
        "limits":{"metadata_budget_bytes":STORE_BUDGET,"scanner_workers":1,"automatic_deletion":false,"UI":"deferred"}})
}
fn run() -> Result<u8> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let command = args.first().map(utf8).transpose()?.unwrap_or("--help");
    let options = parse(command, args.get(1..).unwrap_or_default())?;
    // Only this process's scan/query is interrupted; no existing user process.
    unsafe {
        libc::signal(libc::SIGINT, cancel as *const () as libc::sighandler_t);
    }
    let mut code = 0;
    let result = match command {
        "--help" | "-h" | "schema" => schema(),
        "--version" => {
            json!({"schema_version":"2.0","engine_version":ENGINE_VERSION,"duckdb_version":Store::version()})
        }
        "capabilities" => {
            let mut v = capabilities();
            v["storage"] = json!("parquet_zstd");
            v["query_engine"] = json!(Store::version());
            v["agent_response_budget_bytes"] = json!(OUTPUT_BUDGET);
            v
        }
        "volumes" => volumes(options.query.limit, options.query.cursor.as_deref())
            .map_err(|e| Error::io("volumes", e))?,
        "scan" => {
            if options.operands.len() != 1 {
                return Err(Error::usage("scan requires exactly one explicit ROOT"));
            }
            let mut scan_options = options.scan;
            if options.save {
                scan_options.exclusions.push(options.store.clone());
            }
            let data=scan_with_progress(&PathBuf::from(&options.operands[0]),&scan_options,&CANCEL,|p|{
                match options.progress.as_str() {
                    "ndjson"=>{let _=writeln!(io::stderr(),"{}",json!({"type":"progress","entries_seen":p.entries_seen,"elapsed_ms":p.elapsed_ms,"finished":p.finished}));},
                    "text"=>{let _=writeln!(io::stderr(),"{} entries, {} ms{}",p.entries_seen,p.elapsed_ms,if p.finished {"; scan finished"}else{""});},_=>{}
                }
            }).map_err(|e|Error::io("scan",e))?;
            code = data.status.exit_code();
            let retain = options.save && !CANCEL.load(Ordering::Relaxed);
            let mut result = if retain {
                let store =
                    Store::open(&options.store, true).map_err(|e| Error::io("open_store", e))?;
                store
                    .save_with_cancel(&data, options.metadata_budget, &CANCEL)
                    .map_err(|e| Error::io("save_parquet", e))?;
                let id = data.id.clone();
                drop(data);
                store
                    .report(&id, &options.query, &CANCEL)
                    .map_err(|e| Error::io("query", e))?
            } else {
                compact_scan(&data, options.query).map_err(|e| Error::io("query", e))?
            };
            result["command"] = json!("scan");
            result["retained"] = json!(retain);
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
            store
                .query_sql(
                    id,
                    options.before.as_deref(),
                    sql,
                    options.query.limit,
                    options.offset,
                    &CANCEL,
                )
                .map_err(|e| Error::io("query", e))?
        }
        "compare" => {
            if options.operands.len() != 2 {
                return Err(Error::usage("compare requires BEFORE_ID AFTER_ID"));
            }
            let store =
                Store::open(&options.store, false).map_err(|e| Error::io("open_store", e))?;
            store
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
                .map_err(|e| Error::io("compare", e))?
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
            store
                .report(id, &query, &CANCEL)
                .map_err(|e| Error::io("query", e))?
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
    let result =
        macscan_analysis::compact_response(result, None).map_err(|e| Error::io("response", e))?;
    emit(result, options.ndjson).map_err(|e| Error::io("stdout", e))?;
    Ok(code)
}
fn compact_scan(data: &macscan_engine::Scan, query: Query) -> io::Result<Value> {
    let metric = if query.metric == Metric::Logical {
        "logical_bytes"
    } else {
        "accounted_allocated_bytes"
    };
    let full = data.query(&query)?;
    let mut result = json!({"schema_version":"2.0","engine_version":ENGINE_VERSION,"scan_id":data.id,"status":data.status.name(),"metric":query.metric.name(),
        "scope":{"root":data.root.to_string_lossy()},"coverage":full["coverage"],"totals":{
            "logical_bytes":full["totals"]["logical_bytes"],"accounted_allocated_bytes":full["totals"]["accounted_allocated_bytes"],
            "private_bytes":null,"observed_lower_bound":full["totals"]["observed_lower_bound"]},
        "records":[],"truncated":full["truncated"],"next_cursor":null,"navigation_note":"Use --save for Parquet history, queries and comparison."});
    for entry in full["records"].as_array().into_iter().flatten() {
        let record = json!({"entry_id":entry["entry_id"],"name_display":entry["name_display"],"kind":entry["kind"],"bytes":entry[metric],
            "coverage_complete":entry["coverage_complete"],"lower_bound_bytes":if entry["coverage_complete"]==false {entry["observed_lower_bound"][metric].clone()}else{Value::Null}});
        result["records"].as_array_mut().unwrap().push(record);
        if serde_json::to_vec(&result)?.len() + 512 > OUTPUT_BUDGET {
            result["records"].as_array_mut().unwrap().pop();
            result["truncated"] = json!(true);
            break;
        }
    }
    Ok(result)
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
fn main() -> std::process::ExitCode {
    let code = match run() {
        Ok(code) => code,
        Err(e) => {
            let error = json!({"schema_version":"2.0","engine_version":ENGINE_VERSION,"status":"failed",
                "error":{"code":e.code,"operation":e.operation,"detail":e.detail,"retryable":false}});
            let _ = writeln!(io::stderr(), "{error}");
            e.exit
        }
    };
    std::process::ExitCode::from(code)
}

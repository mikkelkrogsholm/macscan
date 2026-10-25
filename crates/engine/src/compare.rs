//! Bounded streaming comparison of derived preorder metadata indexes.
//! Paths are compared by byte components, so a directory's descendants remain
//! contiguous even when a sibling name starts with punctuation (a, a!, a/z).
use crate::query::invalid;
use crate::scan::hex;
use crate::store::{read_record, unhex};
use crate::{Metric, ENGINE_VERSION, OUTPUT_BUDGET, SCHEMA_VERSION};
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::io::{self, BufRead};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

pub struct CompareQuery {
    pub metric: Metric,
    pub limit: usize,
    pub cursor: Option<String>,
}
impl Default for CompareQuery {
    fn default() -> Self {
        Self {
            metric: Metric::Allocated,
            limit: 30,
            cursor: None,
        }
    }
}

pub(crate) fn path_order(a: &[u8], b: &[u8]) -> Ordering {
    a.split(|c| *c == b'/').cmp(b.split(|c| *c == b'/'))
}
fn descendant(path: &[u8], parent: &[u8]) -> bool {
    parent.is_empty() || (path.starts_with(parent) && path.get(parent.len()) == Some(&b'/'))
}

struct Row {
    path: Vec<u8>,
    id: String,
    kind: String,
    logical: Option<u64>,
    allocated: Option<u64>,
    enumeration_complete: bool,
}
impl Row {
    fn value(&self, metric: Metric) -> Option<u64> {
        match metric {
            Metric::Logical => self.logical,
            Metric::Allocated => self.allocated,
            Metric::Private => None,
        }
    }
}

struct Reader<R: BufRead> {
    input: R,
    expected: usize,
    seen: usize,
    previous: Option<Vec<u8>>,
    ancestors: Vec<(Vec<u8>, bool)>,
}
impl<R: BufRead> Reader<R> {
    fn new(input: R, expected: usize) -> Self {
        Self {
            input,
            expected,
            seen: 0,
            previous: None,
            ancestors: Vec::new(),
        }
    }
    fn next(&mut self) -> io::Result<Option<Row>> {
        let Some(value) = read_record(&mut self.input)? else {
            if self.seen != self.expected {
                return Err(invalid("truncated comparison index"));
            }
            return Ok(None);
        };
        if self.seen >= self.expected {
            return Err(invalid("unexpected index row"));
        }
        let row = value
            .as_array()
            .filter(|r| r.len() == 6)
            .ok_or_else(|| invalid("invalid comparison row"))?;
        let path = unhex(
            row[0]
                .as_str()
                .ok_or_else(|| invalid("invalid comparison path"))?,
        )?;
        if path.contains(&0)
            || path.starts_with(b"/")
            || path.ends_with(b"/")
            || (!path.is_empty()
                && path
                    .split(|b| *b == b'/')
                    .any(|p| p.is_empty() || p == b"." || p == b".."))
            || path.split(|b| *b == b'/').count() > 64
            || path.split(|b| *b == b'/').any(|p| p.len() > 1024)
            || (self.seen == 0 && !path.is_empty())
            || (self.seen > 0 && path.is_empty())
        {
            return Err(invalid("unsafe comparison index path"));
        }
        if self
            .previous
            .as_ref()
            .is_some_and(|p| path_order(p, &path) != Ordering::Less)
        {
            return Err(invalid("unsorted/duplicate comparison index path"));
        }
        self.previous = Some(path.clone());
        self.seen += 1;
        let id = row[1]
            .as_str()
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| invalid("invalid index entry ID"))?;
        if id.parse::<usize>().ok().is_none_or(|n| n >= self.expected) {
            return Err(invalid("index entry ID out of scope"));
        }
        let kind = row[2]
            .as_str()
            .filter(|s| ["directory", "file", "symlink", "special"].contains(s))
            .ok_or_else(|| invalid("invalid indexed kind"))?;
        let number = |v: &Value| -> io::Result<Option<u64>> {
            if v.is_null() {
                Ok(None)
            } else {
                let s = v
                    .as_str()
                    .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                    .ok_or_else(|| invalid("invalid indexed bytes"))?;
                s.parse()
                    .map(Some)
                    .map_err(|_| invalid("indexed bytes overflow"))
            }
        };
        Ok(Some(Row {
            path,
            id: id.into(),
            kind: kind.into(),
            logical: number(&row[3])?,
            allocated: number(&row[4])?,
            enumeration_complete: row[5]
                .as_bool()
                .ok_or_else(|| invalid("invalid indexed coverage"))?,
        }))
    }
    fn consume(&mut self, row: &Row) {
        while self
            .ancestors
            .last()
            .is_some_and(|(p, _)| !descendant(&row.path, p))
        {
            self.ancestors.pop();
        }
        if row.kind == "directory" {
            self.ancestors
                .push((row.path.clone(), row.enumeration_complete));
        }
    }
    fn missing_is_known(&self, path: &[u8]) -> bool {
        self.ancestors
            .iter()
            .rev()
            .find(|(p, _)| descendant(path, p))
            .is_some_and(|(_, complete)| *complete)
    }
}

#[derive(Eq, PartialEq)]
struct Rank {
    unknown: bool,
    magnitude: u64,
    path: Vec<u8>,
}
impl Ord for Rank {
    fn cmp(&self, other: &Self) -> Ordering {
        (!self.unknown)
            .cmp(&(!other.unknown))
            .then_with(|| other.magnitude.cmp(&self.magnitude))
            .then_with(|| path_order(&self.path, &other.path))
    }
}
impl PartialOrd for Rank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
struct Change {
    rank: Rank,
    before: Option<Row>,
    after: Option<Row>,
    before_bytes: Option<u64>,
    after_bytes: Option<u64>,
    delta: Option<i128>,
    kind: &'static str,
}
impl PartialEq for Change {
    fn eq(&self, other: &Self) -> bool {
        self.rank == other.rank
    }
}
impl Eq for Change {}
impl Ord for Change {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank.cmp(&other.rank)
    }
}
impl PartialOrd for Change {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn incompatible(detail: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, detail)
}
fn metadata_count(m: &Value) -> io::Result<usize> {
    m["entry_count"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 1_000_000)
        .map(|n| n as usize)
        .ok_or_else(|| invalid("invalid manifest entry count"))
}

pub(crate) fn compare(
    before: &Value,
    after: &Value,
    before_rows: impl BufRead,
    after_rows: impl BufRead,
    q: &CompareQuery,
    cancel: &AtomicBool,
) -> io::Result<Value> {
    if q.limit == 0 || q.limit > 500 {
        return Err(invalid("comparison limit must be 1..500"));
    }
    if q.metric == Metric::Private {
        return Err(incompatible("private metric is not validated"));
    }
    for field in ["root_bytes_hex", "policy"] {
        if !before[field].is_string() || before[field] != after[field] {
            return Err(incompatible("different roots or exclusion/dedupe policies"));
        }
    }
    if !before["context"]["os_identity"].is_string()
        || before["context"]["os_identity"] != after["context"]["os_identity"]
    {
        return Err(incompatible("different or unknown OS identities"));
    }
    let uuid = &before["context"]["volume_before"]["volume_uuid"];
    if !uuid.is_string()
        || [before, after].iter().any(|m| {
            m["context"]["volume_before"]["volume_uuid"] != *uuid
                || m["context"]["volume_after"]["volume_uuid"] != *uuid
        })
    {
        return Err(incompatible(
            "different, changed or unknown volume identity",
        ));
    }
    let parse_time = |m: &Value| {
        m["context"]["started_unix_ns"]
            .as_str()
            .and_then(|s| s.parse::<u128>().ok())
            .ok_or_else(|| incompatible("unknown observation time"))
    };
    let start_before = parse_time(before)?;
    let start_after = parse_time(after)?;
    let interval = start_after
        .checked_sub(start_before)
        .filter(|n| *n > 0)
        .ok_or_else(|| incompatible("AFTER must start after BEFORE"))?;
    let windows_overlap = before["context"]["ended_unix_ns"]
        .as_str()
        .and_then(|s| s.parse::<u128>().ok())
        .map(|end| end > start_after);
    let rate_available = windows_overlap == Some(false);
    let before_id = before["scan_id"]
        .as_str()
        .ok_or_else(|| invalid("missing before ID"))?;
    let after_id = after["scan_id"]
        .as_str()
        .ok_or_else(|| invalid("missing after ID"))?;
    let binding = format!("compare-v1:{before_id}:{after_id}:{}", q.metric.name());
    let cursor = match &q.cursor {
        None => None,
        Some(text) => {
            let rest = text
                .strip_prefix(&format!("{binding}:"))
                .ok_or_else(|| invalid("cursor belongs to another scan pair/metric"))?;
            let fields: Vec<_> = rest.split(':').collect();
            if fields.len() != 3 {
                return Err(invalid("malformed comparison cursor"));
            }
            Some(Rank {
                unknown: match fields[0] {
                    "0" => true,
                    "1" => false,
                    _ => return Err(invalid("invalid cursor rank")),
                },
                magnitude: fields[1]
                    .parse()
                    .map_err(|_| invalid("invalid cursor delta"))?,
                path: unhex(fields[2])?,
            })
        }
    };
    let mut left = Reader::new(before_rows, metadata_count(before)?);
    let mut right = Reader::new(after_rows, metadata_count(after)?);
    let mut a = left.next()?;
    let mut b = right.next()?;
    let mut heap: BinaryHeap<Change> = BinaryHeap::new();
    let mut matching = 0_u64;
    let mut after_cursor = 0_u64;
    let mut root_before = None;
    let mut root_after = None;
    while a.is_some() || b.is_some() {
        if cancel.load(AtomicOrdering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "comparison cancelled",
            ));
        }
        let order = match (&a, &b) {
            (Some(x), Some(y)) => path_order(&x.path, &y.path),
            (Some(_), None) => Ordering::Less,
            _ => Ordering::Greater,
        };
        let (old, new, old_known, new_known) = match order {
            Ordering::Equal => {
                let old = a.take().unwrap();
                let new = b.take().unwrap();
                left.consume(&old);
                right.consume(&new);
                a = left.next()?;
                b = right.next()?;
                (Some(old), Some(new), true, true)
            }
            Ordering::Less => {
                let old = a.take().unwrap();
                let known = right.missing_is_known(&old.path);
                left.consume(&old);
                a = left.next()?;
                (Some(old), None, true, known)
            }
            Ordering::Greater => {
                let new = b.take().unwrap();
                let known = left.missing_is_known(&new.path);
                right.consume(&new);
                b = right.next()?;
                (None, Some(new), known, true)
            }
        };
        let old_bytes = old
            .as_ref()
            .map(|r| r.value(q.metric))
            .unwrap_or(if old_known { Some(0) } else { None });
        let new_bytes = new
            .as_ref()
            .map(|r| r.value(q.metric))
            .unwrap_or(if new_known { Some(0) } else { None });
        let path = old.as_ref().or(new.as_ref()).unwrap().path.clone();
        if path.is_empty() {
            root_before = old_bytes;
            root_after = new_bytes;
        }
        let delta = old_bytes.zip(new_bytes).map(|(x, y)| y as i128 - x as i128);
        let kind = if delta.is_none() {
            "not_observed_or_incomplete"
        } else if old.is_none() {
            "added"
        } else if new.is_none() {
            "removed"
        } else if old.as_ref().unwrap().kind != new.as_ref().unwrap().kind {
            "type_changed"
        } else if delta.unwrap() > 0 {
            "grown"
        } else if delta.unwrap() < 0 {
            "shrunk"
        } else {
            continue;
        };
        if path.is_empty() && old.as_ref().or(new.as_ref()).unwrap().kind == "directory" {
            continue;
        }
        matching += 1;
        let rank = Rank {
            unknown: delta.is_none(),
            magnitude: delta.map(|n| n.unsigned_abs() as u64).unwrap_or(0),
            path,
        };
        if cursor.as_ref().is_some_and(|c| rank <= *c) {
            continue;
        }
        after_cursor += 1;
        let change = Change {
            rank,
            before: old,
            after: new,
            before_bytes: old_bytes,
            after_bytes: new_bytes,
            delta,
            kind,
        };
        if heap.len() < q.limit {
            heap.push(change);
        } else if heap.peek().is_some_and(|worst| change < *worst) {
            heap.pop();
            heap.push(change);
        }
    }
    let root_delta = root_before
        .zip(root_after)
        .map(|(x, y)| y as i128 - x as i128);
    let coverage_complete = before["status"] == "complete" && after["status"] == "complete";
    let mut response = json!({"schema_version":SCHEMA_VERSION,"engine_version":ENGINE_VERSION,"scan_id":after_id,
        "before_scan_id":before_id,"status":if coverage_complete { "complete" } else { "partial" },"command":"compare",
        "scope":{"root_path_bytes_hex":before["root_bytes_hex"],"policy_fingerprint":before["policy"],"volume_uuid":uuid},
        "metric":q.metric.name(),"coverage":{"complete":coverage_complete,"before_status":before["status"],"after_status":after["status"]},
        "compatibility":"same_root_os_volume_and_policy; per-path coverage checked",
        "totals":{"before_bytes":root_before.map(|n|n.to_string()),"after_bytes":root_after.map(|n|n.to_string()),"delta_bytes":root_delta.map(|n|n.to_string())},
        "observation_interval":{"before_started_at":before["started_at"],"after_started_at":after["started_at"],"start_to_start_ns":interval.to_string(),"windows_overlap":windows_overlap},
        "average_rate":{"numerator_bytes":if rate_available {root_delta.map(|n|n.to_string())}else{None},"denominator_ns":interval.to_string(),"unit":"bytes/ns","quality":if rate_available {"estimated"}else{"unavailable"},"meaning":"average_between_observations_not_measured_write_speed"},
        "capacity_before":before["context"]["volume_before"],"capacity_after":after["context"]["volume_after"],
        "records":[],"matching_records":matching,"truncated":false,"next_cursor":null,
        "warnings":[{"code":"NESTED_CHANGES_OVERLAP_DO_NOT_SUM"},{"code":"SHARED_APFS_BLOCKS_NOT_DEDUPLICATED"},
            {"code":"COVERAGE_GAPS_ARE_NOT_DELETIONS"},{"code":"METADATA_NOT_CONTENT_COMPARISON_NO_PROCESS_ATTRIBUTION"}]});
    if !rate_available {
        response["warnings"]
            .as_array_mut()
            .unwrap()
            .push(json!({"code":"OVERLAPPING_OR_UNKNOWN_OBSERVATION_WINDOWS_RATE_UNAVAILABLE"}));
    }
    let root = unhex(before["root_bytes_hex"].as_str().unwrap())?;
    let root_path = Path::new(std::ffi::OsStr::from_bytes(&root));
    if !root_path.is_absolute() || root.contains(&0) {
        return Err(invalid("invalid saved root locator"));
    }
    let mut emitted = 0_usize;
    let mut last_cursor = None;
    for change in heap.into_sorted_vec() {
        let full = root_path.join(std::ffi::OsStr::from_bytes(&change.rank.path));
        let cursor = format!(
            "{binding}:{}:{}:{}",
            if change.rank.unknown { 0 } else { 1 },
            change.rank.magnitude,
            hex(&change.rank.path)
        );
        let record = json!({"path_display":full.to_string_lossy(),"path_bytes_hex":hex(full.as_os_str().as_bytes()),
            "before_entry_id":change.before.as_ref().map(|r|r.id.as_str()),"after_entry_id":change.after.as_ref().map(|r|r.id.as_str()),
            "kind_before":change.before.as_ref().map(|r|r.kind.as_str()),"kind_after":change.after.as_ref().map(|r|r.kind.as_str()),
            "before_bytes":change.before_bytes.map(|n|n.to_string()),"after_bytes":change.after_bytes.map(|n|n.to_string()),
            "delta_bytes":change.delta.map(|n|n.to_string()),"change":change.kind,"quality":if change.delta.is_some(){"measured_metadata"}else{"unavailable"}});
        response["records"].as_array_mut().unwrap().push(record);
        response["next_cursor"] = json!(cursor);
        if serde_json::to_vec(&response)?.len() + 4096 > OUTPUT_BUDGET {
            response["records"].as_array_mut().unwrap().pop();
            if emitted == 0 {
                return Err(invalid("one comparison record exceeds output budget"));
            }
            break;
        }
        emitted += 1;
        last_cursor = Some(cursor);
    }
    response["truncated"] = json!((emitted as u64) < after_cursor);
    response["next_cursor"] = if (emitted as u64) < after_cursor {
        json!(last_cursor)
    } else {
        Value::Null
    };
    if serde_json::to_vec(&response)?.len() + 1 > OUTPUT_BUDGET {
        return Err(invalid("comparison metadata exceeds output budget"));
    }
    Ok(response)
}

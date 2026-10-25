//! Shared bounded result contract. CLI and Tauri use this exact code.
use crate::{Entry, EntryKind, Scan};
use serde_json::{json, Value};
use std::io;

pub const OUTPUT_BUDGET: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    Logical,
    Allocated,
    Private,
}
impl Metric {
    pub fn parse(value: &str) -> io::Result<Self> {
        match value {
            "logical" => Ok(Self::Logical),
            "allocated" => Ok(Self::Allocated),
            "private" => Ok(Self::Private),
            _ => Err(invalid("unknown metric")),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Logical => "logical",
            Self::Allocated => "allocated",
            Self::Private => "private",
        }
    }
    pub fn value(self, e: &Entry) -> Option<u64> {
        if !e.complete {
            return None;
        }
        match self {
            Self::Logical => Some(e.logical),
            Self::Allocated => Some(e.accounted),
            Self::Private => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryKind {
    Summary,
    Children,
    Largest,
    Inspect,
}
impl QueryKind {
    fn name(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Children => "children",
            Self::Largest => "largest",
            Self::Inspect => "inspect",
        }
    }
}

pub struct Query {
    pub kind: QueryKind,
    pub entry: u32,
    pub metric: Metric,
    pub limit: usize,
    pub cursor: Option<String>,
    pub search: String,
    pub only_kind: Option<EntryKind>,
    pub min_bytes: u64,
}
impl Default for Query {
    fn default() -> Self {
        Self {
            kind: QueryKind::Children,
            entry: 0,
            metric: Metric::Allocated,
            limit: 30,
            cursor: None,
            search: String::new(),
            only_kind: None,
            min_bytes: 0,
        }
    }
}

pub(crate) fn invalid(detail: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, detail)
}

impl Scan {
    pub fn query(&self, q: &Query) -> io::Result<Value> {
        if q.limit == 0 || q.limit > 500 || q.search.len() > 1024 {
            return Err(invalid("limit must be 1..500 and search <=1024 bytes"));
        }
        if q.entry as usize >= self.entries.len() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "entry not found"));
        }
        if q.metric == Metric::Private {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "private metric is not validated on this backend",
            ));
        }
        if matches!(q.kind, QueryKind::Inspect | QueryKind::Summary) && q.cursor.is_some() {
            return Err(invalid("this command does not accept a cursor"));
        }
        let binding = format!(
            "v1:{}:{}:{}:{}:{}:{}:{}",
            self.id,
            q.kind.name(),
            q.entry,
            q.metric.name(),
            crate::scan::hex(q.search.as_bytes()),
            q.only_kind.map(|k| k.name()).unwrap_or("all"),
            q.min_bytes
        );
        let offset = match &q.cursor {
            None => 0,
            Some(c) => {
                let (prefix, offset) = c
                    .rsplit_once(':')
                    .ok_or_else(|| invalid("malformed cursor"))?;
                if prefix != binding {
                    return Err(invalid("cursor belongs to a different scan/query/metric"));
                }
                offset
                    .parse::<usize>()
                    .map_err(|_| invalid("invalid cursor offset"))?
            }
        };
        let mut response = self.report(0);
        response["command"] = json!(q.kind.name());
        response["metric"] = json!(q.metric.name());
        response["query_entry_id"] = json!(q.entry.to_string());
        response["totals"] = self.entry_json(q.entry);
        response["records"] = json!([]);
        response["truncated"] = json!(false);
        response["next_cursor"] = Value::Null;
        // Diagnostic samples are bounded independently; total counts remain exact.
        response["issues"] =
            json!(self.issues.iter().take(8).map(|i| json!({
            "entry_id": i.entry_id.to_string(), "code": i.code, "operation": i.operation,
            "detail": i.detail, "child_name_bytes_hex": i.name.as_deref().map(crate::scan::hex)
        })).collect::<Vec<_>>());
        response["coverage"]["issue_samples_truncated"] =
            json!(self.issue_count > self.issues.len().min(8) as u64);

        if q.kind == QueryKind::Summary {
            return ensure_budget(response);
        }
        if q.kind == QueryKind::Inspect {
            response["records"] = json!([self.entry_json(q.entry)]);
            return ensure_budget(response);
        }
        let mut ids: Vec<u32> = Vec::new();
        for (id, e) in self.entries.iter().enumerate().skip(1) {
            let id = id as u32;
            let included = match q.kind {
                QueryKind::Children => e.parent == Some(q.entry),
                QueryKind::Largest => e.kind == EntryKind::File && self.is_descendant(id, q.entry),
                _ => false,
            };
            if included
                && q.only_kind.is_none_or(|k| k == e.kind)
                && (q.min_bytes == 0 || q.metric.value(e).is_some_and(|n| n >= q.min_bytes))
                && (q.search.is_empty() || self.path(id).to_string_lossy().contains(&q.search))
            {
                ids.push(id);
            }
        }
        // Unknown totals sort first so inaccessible directories do not disappear.
        ids.sort_unstable_by_key(|id| {
            (
                q.metric
                    .value(&self.entries[*id as usize])
                    .map(std::cmp::Reverse),
                *id,
            )
        });
        if offset > ids.len() {
            return Err(invalid("cursor is beyond end of result"));
        }
        let mut records = Vec::new();
        let mut next = offset;
        for id in ids.iter().skip(offset).take(q.limit) {
            let record = self.entry_json(*id);
            records.push(record);
            response["records"] = json!(records);
            response["truncated"] = json!(true);
            response["next_cursor"] = json!(format!("{binding}:{}", next + 1));
            // Reserve framing space for the CLI's NDJSON start/record/end wrapper.
            if serde_json::to_vec(&response)?.len() + 4096 > OUTPUT_BUDGET {
                records.pop();
                if records.is_empty() {
                    return Err(invalid(
                        "one record exceeds output budget; locator was not truncated",
                    ));
                }
                break;
            }
            next += 1;
        }
        response["records"] = json!(records);
        response["truncated"] = json!(next < ids.len());
        response["next_cursor"] = if next < ids.len() {
            json!(format!("{binding}:{next}"))
        } else {
            Value::Null
        };
        response["matching_records"] = json!(ids.len());
        ensure_budget(response)
    }

    fn is_descendant(&self, mut id: u32, ancestor: u32) -> bool {
        while let Some(parent) = self.entries[id as usize].parent {
            if parent == ancestor {
                return true;
            }
            id = parent;
        }
        false
    }
}

fn ensure_budget(value: Value) -> io::Result<Value> {
    if serde_json::to_vec(&value)?.len() + 1 > OUTPUT_BUDGET {
        return Err(invalid("result metadata exceeds output budget"));
    }
    Ok(value)
}

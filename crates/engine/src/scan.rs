use crate::platform::{self, Directory, Metadata};
use crate::{ENGINE_VERSION, SCHEMA_VERSION};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    File,
    Symlink,
    Special,
}
impl EntryKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::File => "file",
            Self::Symlink => "symlink",
            Self::Special => "special",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanStatus {
    Complete,
    Partial,
    Cancelled,
}
impl ScanStatus {
    pub fn name(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Cancelled => "cancelled",
        }
    }
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Complete => 0,
            Self::Partial => 5,
            Self::Cancelled => 130,
        }
    }
}

#[derive(Debug)]
pub struct Entry {
    pub parent: Option<u32>,
    pub name: Vec<u8>,
    pub kind: EntryKind,
    pub logical: u64,
    pub allocated: u64,
    pub accounted: u64,
    pub own_allocated: u64,
    pub device: u64,
    pub inode: u64,
    pub links: u64,
    pub observed_links: u32,
    pub charged_to: Option<u32>,
    pub mtime: i64,
    pub complete: bool,
    pub enumeration_complete: bool,
}

impl Entry {
    fn new(parent: Option<u32>, name: Vec<u8>, metadata: Metadata) -> Self {
        let kind = if metadata.is_dir() {
            EntryKind::Directory
        } else if metadata.is_file() {
            EntryKind::File
        } else if metadata.is_symlink() {
            EntryKind::Symlink
        } else {
            EntryKind::Special
        };
        Self {
            parent,
            name,
            kind,
            logical: if kind == EntryKind::Directory {
                0
            } else {
                metadata.logical
            },
            allocated: metadata.allocated,
            accounted: metadata.allocated,
            own_allocated: metadata.allocated,
            device: metadata.device,
            inode: metadata.inode,
            links: metadata.links,
            observed_links: 1,
            charged_to: None,
            mtime: metadata.mtime,
            complete: true,
            enumeration_complete: true,
        }
    }
}

#[derive(Debug)]
pub struct Issue {
    pub entry_id: u32,
    pub code: String,
    pub operation: String,
    pub detail: String,
    pub name: Option<Vec<u8>>,
}

pub struct ScanOptions {
    pub max_entries: usize,
    pub max_duration: Duration,
    pub max_depth: usize,
    pub max_memory_bytes: usize,
    pub exclusions: Vec<PathBuf>,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            max_entries: 1_000_000,
            max_duration: Duration::from_secs(300),
            max_depth: 64,
            max_memory_bytes: 224 * 1024 * 1024,
            exclusions: Vec::new(),
        }
    }
}

pub struct Scan {
    pub id: String,
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    pub issues: Vec<Issue>,
    pub issue_count: u64,
    pub excluded_count: u64,
    pub status: ScanStatus,
    pub started_at: String,
    pub ended_at: String,
    pub duration_ms: u128,
    pub policy: String,
    pub context: Value,
}

pub fn capabilities() -> Value {
    json!({"schema_version": SCHEMA_VERSION, "engine_version": ENGINE_VERSION,
        "platform": "macos", "backend": "darwin_descriptor_relative",
        "implementation_stage": "engine_prototype", "read_only_targets": true,
        "supported_metrics": ["logical", "allocated"], "private_bytes": null,
        "clone_metadata": "not_yet_implemented", "atomic_filesystem_snapshot": false,
        "network": false, "cleanup": false, "workers": 1,
        "warnings": ["SHARED_APFS_BLOCKS_NOT_DEDUPLICATED", "V1_ACCEPTANCE_INCOMPLETE"]})
}

pub fn scan(root: &Path, options: &ScanOptions, cancel: &AtomicBool) -> io::Result<Scan> {
    scan_with_progress(root, options, cancel, |_| {})
}

pub struct Progress {
    pub entries_seen: usize,
    pub elapsed_ms: u128,
    pub finished: bool,
}

pub fn scan_with_progress(
    root: &Path,
    options: &ScanOptions,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&Progress),
) -> io::Result<Scan> {
    let started = Instant::now();
    let started_at = platform::timestamp();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    progress(&Progress {
        entries_seen: 0,
        elapsed_ms: 0,
        finished: false,
    });
    if options.max_entries == 0 || options.max_depth == 0 || options.max_depth > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "entry/depth limits must be positive",
        ));
    }
    let requested_root = if root.is_absolute() {
        root.to_owned()
    } else {
        std::env::current_dir()?.join(root)
    };
    let initial = requested_root.symlink_metadata()?;
    if initial.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "symlink roots must be explicitly resolved",
        ));
    }
    let metadata = Metadata::from_std(&initial)?;
    // Known provider roots are rejected before canonicalization or directory open.
    let mut cloud_root = cloud_path(&requested_root) || metadata.dataless();
    let root = if cloud_root {
        requested_root
    } else {
        requested_root.canonicalize()?
    };
    // An ancestor symlink can resolve into a known provider subtree. Recheck
    // the canonical path before any directory open/enumeration.
    cloud_root |= cloud_path(&root);
    let directory = if metadata.is_dir() && !cloud_root {
        let directory = Directory::root(&root)?;
        let opened = directory.metadata()?;
        if (opened.device, opened.inode) != (metadata.device, metadata.inode) {
            return Err(io::Error::other("root identity changed during open"));
        }
        Some(directory)
    } else {
        None
    };
    let capacity_directory = if let Some(d) = directory.as_ref() {
        Some(d.child(&CString::new(".").unwrap(), metadata)?)
    } else if !cloud_root && !metadata.is_dir() {
        let parent = Directory::root(
            root.parent()
                .ok_or_else(|| io::Error::other("root has no parent"))?,
        )?;
        if parent.metadata()?.device == metadata.device {
            Some(parent)
        } else {
            None
        }
    } else {
        None
    };
    let capture_capacity = || {
        capacity_directory
            .as_ref()
            .map(|d| crate::volume::capture_fd(d.fd()))
            .transpose()
            .map(|v| v.unwrap_or(Value::Null))
            .unwrap_or_else(|e| json!({"status":"unavailable","detail":e.to_string()}))
    };
    let before_capacity = capture_capacity();
    if before_capacity["local"] == false || before_capacity["supported_local_scan"] == false {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "selected root is not a supported local disk filesystem",
        ));
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut scan = Scan {
        id: format!(
            "{nanos}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ),
        root,
        entries: Vec::new(),
        issues: Vec::new(),
        issue_count: 0,
        excluded_count: 0,
        status: ScanStatus::Complete,
        started_at,
        ended_at: String::new(),
        duration_ms: 0,
        policy: format!(
            "descriptor-v1;depth={};exclusions={}",
            options.max_depth,
            options
                .exclusions
                .iter()
                .map(|p| hex(p.as_os_str().as_bytes()))
                .collect::<Vec<_>>()
                .join(",")
        ),
        context: json!({"os_identity":crate::volume::os_identity().ok(),"volume_before":before_capacity,
            "volume_after":null,"started_unix_ns":nanos.to_string(),"ended_unix_ns":null}),
    };
    scan.entries.push(Entry::new(None, Vec::new(), metadata));
    let mut stack: Vec<_> = directory.into_iter().map(|d| (0_u32, d)).collect();
    if cloud_root || options.exclusions.iter().any(|p| scan.root.starts_with(p)) {
        scan.issue(
            0,
            if cloud_root {
                "CLOUD_EXCLUDED"
            } else {
                "OUTPUT_EXCLUDED"
            },
            "scope",
            "cloud/provider or dataless root excluded".into(),
            true,
        );
        stack.clear();
    }
    if cancel.load(Ordering::Relaxed) {
        scan.status = ScanStatus::Cancelled;
        scan.entries[0].complete = false;
        scan.entries[0].enumeration_complete = false;
        stack.clear();
    }
    let mut name_bytes = 0_usize;
    let mut last_progress = Instant::now();
    while !stack.is_empty() {
        if last_progress.elapsed() >= Duration::from_secs(1) {
            progress(&Progress {
                entries_seen: scan.entries.len(),
                elapsed_ms: started.elapsed().as_millis(),
                finished: false,
            });
            last_progress = Instant::now();
        }
        if cancel.load(Ordering::Relaxed) {
            scan.status = ScanStatus::Cancelled;
            scan.entries[0].complete = false;
            scan.entries[0].enumeration_complete = false;
            break;
        }
        if started.elapsed() >= options.max_duration {
            scan.issue(
                0,
                "SCAN_LIMIT",
                "scan",
                "entry or duration limit reached".into(),
                false,
            );
            break;
        }
        let (parent, directory) = stack.last_mut().expect("nonempty stack");
        let parent = *parent;
        let name = match directory.next_name() {
            Ok(Some(name)) => name,
            Ok(None) => {
                stack.pop();
                continue;
            }
            Err(e) => {
                scan.issue(parent, "ENUMERATION_ERROR", "readdir", e.to_string(), false);
                stack.pop();
                continue;
            }
        };
        if scan.entries.len() >= options.max_entries {
            scan.issue(0, "SCAN_LIMIT", "scan", "entry limit reached".into(), false);
            break;
        }
        let metadata = match directory.child_metadata(&name) {
            Ok(m) => m,
            Err(e) => {
                scan.issue(parent, "METADATA_ERROR", "fstatat", e.to_string(), false);
                if let Some(issue) = scan.issues.last_mut().filter(|_| scan.issue_count <= 128) {
                    issue.name = Some(name.as_bytes().to_vec());
                }
                continue;
            }
        };
        let id =
            u32::try_from(scan.entries.len()).map_err(|_| io::Error::other("too many entries"))?;
        name_bytes = name_bytes.saturating_add(name.as_bytes().len());
        // Reserve room for entries, names, query IDs, and worst-case hardlink map.
        let projected = scan
            .entries
            .len()
            .saturating_add(1)
            .saturating_mul(192)
            .saturating_add(name_bytes);
        if projected > options.max_memory_bytes {
            scan.issue(
                0,
                "MEMORY_LIMIT",
                "scan",
                "metadata memory budget reached".into(),
                false,
            );
            break;
        }
        scan.entries
            .try_reserve(1)
            .map_err(|_| io::Error::other("entry allocation failed"))?;
        scan.entries
            .push(Entry::new(Some(parent), name.as_bytes().to_vec(), metadata));
        if !metadata.is_dir() {
            continue;
        }
        let child_path = scan.path(id);
        if metadata.device != scan.entries[0].device {
            scan.issue(
                id,
                "MOUNT_EXCLUDED",
                "scope",
                "different volume excluded".into(),
                true,
            );
        } else if options.exclusions.iter().any(|p| child_path.starts_with(p)) {
            scan.issue(
                id,
                "OUTPUT_EXCLUDED",
                "scope",
                "explicit metadata/output path excluded".into(),
                true,
            );
        } else if metadata.dataless() || cloud_path(&child_path) {
            scan.issue(
                id,
                "CLOUD_EXCLUDED",
                "scope",
                "cloud/provider or dataless directory excluded".into(),
                true,
            );
        } else if stack.len() >= options.max_depth {
            scan.issue(
                id,
                "DEPTH_LIMIT",
                "scope",
                "maximum open directory depth reached".into(),
                true,
            );
        } else {
            let opened = stack
                .last()
                .expect("parent still open")
                .1
                .child(&name, metadata);
            match opened {
                Ok(child) => stack.push((id, child)),
                Err(e) => scan.issue(id, "OPEN_DIRECTORY_ERROR", "openat", e.to_string(), false),
            }
        }
    }
    // Any frame still open at a resource limit/cancellation was not exhausted.
    // Mark every such directory unknown, not just the root summary.
    for (id, _) in &stack {
        scan.entries[*id as usize].complete = false;
        scan.entries[*id as usize].enumeration_complete = false;
    }
    drop(stack);
    scan.deduplicate_links();
    scan.aggregate()?;
    scan.context["volume_after"] = capture_capacity();
    scan.context["ended_unix_ns"] = json!(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string());
    if scan.context["volume_before"]["fsid"].is_string()
        && scan.context["volume_after"]["fsid"] != scan.context["volume_before"]["fsid"]
    {
        scan.issue(
            0,
            "VOLUME_CHANGED",
            "fstatfs",
            "volume identity or availability changed during scan".into(),
            false,
        );
    }
    scan.ended_at = platform::timestamp();
    scan.duration_ms = started.elapsed().as_millis();
    progress(&Progress {
        entries_seen: scan.entries.len(),
        elapsed_ms: scan.duration_ms,
        finished: true,
    });
    Ok(scan)
}

fn cloud_path(path: &Path) -> bool {
    let bytes = path.as_os_str().as_bytes();
    [
        b"/Library/CloudStorage".as_slice(),
        b"/Library/Mobile Documents".as_slice(),
    ]
    .iter()
    .any(|needle| {
        bytes.windows(needle.len()).enumerate().any(|(i, w)| {
            w == *needle && (i + needle.len() == bytes.len() || bytes[i + needle.len()] == b'/')
        })
    })
}

impl Scan {
    fn issue(
        &mut self,
        id: u32,
        code: &'static str,
        operation: &'static str,
        detail: String,
        excluded: bool,
    ) {
        self.issue_count += 1;
        self.excluded_count += u64::from(excluded);
        self.entries[id as usize].complete = false;
        self.entries[id as usize].enumeration_complete = false;
        if self.issues.len() < 128 {
            self.issues.push(Issue {
                entry_id: id,
                code: code.into(),
                operation: operation.into(),
                detail,
                name: None,
            });
        }
        if self.status != ScanStatus::Cancelled {
            self.status = ScanStatus::Partial;
        }
    }

    pub fn path(&self, id: u32) -> PathBuf {
        let mut parts = Vec::new();
        let mut current = id;
        while let Some(parent) = self.entries[current as usize].parent {
            parts.push(&self.entries[current as usize].name);
            current = parent;
        }
        let mut path = self.root.clone();
        for part in parts.into_iter().rev() {
            path.push(std::ffi::OsStr::from_bytes(part));
        }
        path
    }

    fn deduplicate_links(&mut self) {
        let mut groups: HashMap<(u64, u64), (u32, u32)> = HashMap::new();
        for (id, entry) in self.entries.iter().enumerate() {
            if entry.kind == EntryKind::File && entry.links > 1 {
                let group = groups
                    .entry((entry.device, entry.inode))
                    .or_insert((id as u32, 0));
                group.1 += 1;
                if self.path(id as u32).as_os_str().as_bytes()
                    < self.path(group.0).as_os_str().as_bytes()
                {
                    group.0 = id as u32;
                }
            }
        }
        for (id, entry) in self.entries.iter_mut().enumerate() {
            if entry.kind == EntryKind::File && entry.links > 1 {
                let (canonical, count) = groups[&(entry.device, entry.inode)];
                entry.observed_links = count;
                if id as u32 != canonical {
                    entry.accounted = 0;
                    entry.charged_to = Some(canonical);
                }
            }
        }
    }

    fn aggregate(&mut self) -> io::Result<()> {
        for id in (1..self.entries.len()).rev() {
            let child = &self.entries[id];
            let (parent, logical, allocated, accounted, complete) = (
                child.parent.expect("nonroot has parent") as usize,
                child.logical,
                child.allocated,
                child.accounted,
                child.complete,
            );
            let parent = &mut self.entries[parent];
            parent.logical = parent
                .logical
                .checked_add(logical)
                .ok_or_else(|| io::Error::other("logical total overflow"))?;
            parent.allocated = parent
                .allocated
                .checked_add(allocated)
                .ok_or_else(|| io::Error::other("allocated total overflow"))?;
            parent.accounted = parent
                .accounted
                .checked_add(accounted)
                .ok_or_else(|| io::Error::other("accounted total overflow"))?;
            parent.complete &= complete;
        }
        Ok(())
    }

    pub fn entry_json(&self, id: u32) -> Value {
        let entry = &self.entries[id as usize];
        let path = self.path(id);
        json!({"entry_id": id.to_string(), "parent_id": entry.parent.map(|p| p.to_string()),
            "name_display": String::from_utf8_lossy(&entry.name), "path_display": path.to_string_lossy(),
            "path_bytes_hex": hex(path.as_os_str().as_bytes()), "kind": entry.kind.name(),
            "logical_bytes": entry.complete.then(|| entry.logical.to_string()),
            "allocated_bytes": entry.complete.then(|| entry.allocated.to_string()),
            "accounted_allocated_bytes": entry.complete.then(|| entry.accounted.to_string()), "private_bytes": null,
            "observed_lower_bound": {"logical_bytes": entry.logical.to_string(), "allocated_bytes": entry.allocated.to_string(), "accounted_allocated_bytes": entry.accounted.to_string()},
            "own_allocated_bytes": entry.own_allocated.to_string(),
            "device_id": entry.device.to_string(), "inode": entry.inode.to_string(),
            "link_count": entry.links.to_string(), "observed_link_count": entry.observed_links,
            "charged_to": entry.charged_to.map(|id| id.to_string()), "mtime_unix_seconds": entry.mtime.to_string(),
            "coverage_complete": entry.complete, "allocation_source": "darwin_stat_st_blocks_times_512",
            "enumeration_complete":entry.enumeration_complete,
            "quality": "measured", "private_quality": "unavailable", "reclaimable_bytes": null,
            "streams": "logical=data_fork; allocated=OS_stat_blocks_including_reported_metadata_and_other_forks; no_independent_fork_accounting"})
    }

    pub fn report(&self, limit: usize) -> Value {
        let mut largest: Vec<u32> = if limit == 0 {
            Vec::new()
        } else {
            (1..self.entries.len() as u32).collect()
        };
        largest.sort_unstable_by_key(|id| {
            (std::cmp::Reverse(self.entries[*id as usize].accounted), *id)
        });
        largest.truncate(limit.min(500));
        json!({"schema_version": SCHEMA_VERSION, "engine_version": ENGINE_VERSION,
            "scan_id": self.id, "status": self.status.name(), "implementation_stage": "engine_prototype",
            "scope": {"root_display": self.root.to_string_lossy(), "root_path_bytes_hex": hex(self.root.as_os_str().as_bytes()),
                "same_volume": true, "follow_symlinks": false, "dedup_scope": "root", "policy_fingerprint": self.policy},
            "started_at": self.started_at, "ended_at": self.ended_at, "duration_ms": self.duration_ms,
            "metric": "allocated", "consistency": "live_filesystem_non_atomic",
            "context": self.context,
            "coverage": {"complete": self.entries[0].complete, "error_count": self.issue_count,
                "excluded_count": self.excluded_count, "entries_seen": self.entries.len(),
                "issue_samples_truncated": self.issue_count > self.issues.len() as u64},
            "totals": self.entry_json(0),
            "records": largest.iter().map(|id| self.entry_json(*id)).collect::<Vec<_>>(),
            "truncated": self.entries.len().saturating_sub(1) > largest.len(), "next_cursor": null,
            "warnings": [{"code": "SHARED_APFS_BLOCKS_NOT_DEDUPLICATED"}, {"code": "V1_ACCEPTANCE_INCOMPLETE"}],
            "issues": self.issues.iter().map(|i| json!({"entry_id":i.entry_id.to_string(), "code":i.code, "operation":i.operation, "detail":i.detail, "child_name_bytes_hex": i.name.as_deref().map(hex)})).collect::<Vec<_>>()})
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}

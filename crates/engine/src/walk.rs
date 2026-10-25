//! Descriptor-relative DFS. Only open ancestors are retained, not the full tree.
//! Entries are emitted on completion, with preorder IDs and subtree boundaries.
use crate::platform::{self, Directory, Metadata};
use crate::scan::{cloud_path, hex};
use crate::{Entry, Progress, Scan, ScanOptions, ScanStatus};
use serde_json::json;
use std::ffi::{CString, OsStr};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct StreamEntry {
    pub id: u32,
    pub subtree_end: u32,
    pub path: PathBuf,
    pub relative: PathBuf,
    pub entry: Entry,
}
pub struct WalkSummary {
    /// Header and sampled issues only; entries contains the root, not the tree.
    pub scan: Scan,
    pub entry_count: usize,
}
struct Frame {
    node: StreamEntry,
    directory: Directory,
}
struct Branches(Vec<(Vec<u8>, serde_json::Value)>);
impl Branches {
    fn discover(root:&Path)->Self {
        let mut found=Self(Vec::new());
        if let Ok(mut directory)=Directory::root(root) {
            for _ in 0..4096 {
                match directory.next_metadata() {
                    Ok(Some((name,m))) if m.is_dir()=>{found.slot(name.as_bytes());},
                    Ok(Some(_))=>{}, _=>break,
                }
            }
        }
        found
    }
    fn slot(&mut self,name:&[u8])->usize {
        if let Some(i)=self.0.iter().position(|(n,_)|n==name){return i;}
        let name=if self.0.len()>=63 {b"".as_slice()}else{name};
        if let Some(i)=self.0.iter().position(|(n,_)|n==name){return i;}
        self.0.push((name.to_vec(),json!({"name":if name.is_empty(){"Øvrige rodposter".into()}else{String::from_utf8_lossy(name).into_owned()},"phase":"pending","entries":0,"raw_allocated_bytes":"0","logical_bytes":"0","coverage_complete":true})));
        self.0.len()-1
    }
    fn note(&mut self,n:&StreamEntry) {
        let Some(first)=n.relative.components().next() else{return;};
        let i=self.slot(first.as_os_str().as_bytes());let row=&mut self.0[i].1;
        row["entries"]=json!(row["entries"].as_u64().unwrap_or(0)+1);
        for (key,add) in [("raw_allocated_bytes",n.entry.own_allocated),("logical_bytes",n.entry.logical)] {
            let old=row[key].as_str().unwrap().parse::<u64>().unwrap();row[key]=json!(old.saturating_add(add).to_string());
        }
        row["coverage_complete"]=json!(row["coverage_complete"]==true&&n.entry.enumeration_complete);
        row["phase"]=json!(if n.relative.components().count()==1 {"traversed"}else{"scanning"});
    }
    fn rows(&self)->Vec<serde_json::Value>{self.0.iter().map(|(_,v)|v.clone()).collect()}
}
fn issue(
    scan: &mut Scan,
    node: &mut StreamEntry,
    code: &'static str,
    detail: String,
    excluded: bool,
) {
    node.entry.complete = false;
    node.entry.enumeration_complete = false;
    scan.issue(0, code, "scan", detail, excluded);
    if let Some(sample) = scan.issues.last_mut().filter(|_| scan.issue_count <= 128) {
        sample.entry_id = node.id;
    }
}

pub fn walk_with_progress(
    requested: &Path,
    options: &ScanOptions,
    cancel: &AtomicBool,
    mut emit: impl FnMut(StreamEntry) -> io::Result<()>,
    mut progress: impl FnMut(&Progress),
) -> io::Result<WalkSummary> {
    let started = Instant::now();
    let started_at = platform::timestamp();
    if options.max_entries == 0 || options.max_depth == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "entry/depth limits must be positive",
        ));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    let requested = if requested.is_absolute() {
        requested.to_owned()
    } else {
        std::env::current_dir()?.join(requested)
    };
    let initial = requested.symlink_metadata()?;
    if initial.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "symlink roots must be explicitly resolved",
        ));
    }
    let metadata = Metadata::from_std(&initial)?;
    let mut excluded_root = cloud_path(&requested) || metadata.dataless();
    let root = if excluded_root {
        requested
    } else {
        requested.canonicalize()?
    };
    excluded_root |= cloud_path(&root);
    let directory = if metadata.is_dir() && !excluded_root {
        let d = Directory::root(&root)?;
        let opened = d.metadata()?;
        if (opened.device, opened.inode) != (metadata.device, metadata.inode) {
            return Err(io::Error::other("root identity changed during open"));
        }
        Some(d)
    } else {
        None
    };
    let capacity = if let Some(d) = &directory {
        Some(d.child(&CString::new(".").unwrap(), metadata)?)
    } else if !excluded_root {
        root.parent().map(Directory::root).transpose()?
    } else {
        None
    };
    let capture = || {
        capacity
            .as_ref()
            .map(|d| crate::volume::capture_fd(d.fd()))
            .transpose()
            .map(|v| v.unwrap_or(serde_json::Value::Null))
            .unwrap_or_else(|e| json!({"status":"unavailable","detail":e.to_string()}))
    };
    let before = capture();
    if before["local"] == false || before["supported_local_scan"] == false {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "selected root is not a supported local disk filesystem",
        ));
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut scan = Scan {
        id: format!(
            "{nanos}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ),
        root: root.clone(),
        entries: vec![Entry::new(None, Vec::new(), metadata)],
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
        context: json!({"os_identity":crate::volume::os_identity().ok(),"volume_before":before,
            "volume_after":null,"started_unix_ns":nanos.to_string(),"ended_unix_ns":null}),
    };
    let mut node = StreamEntry {
        id: 0,
        subtree_end: 0,
        path: root.clone(),
        relative: PathBuf::new(),
        entry: Entry::new(None, Vec::new(), metadata),
    };
    let root_excluded = options.exclusions.iter().any(|p| root.starts_with(p));
    if excluded_root || root_excluded {
        issue(
            &mut scan,
            &mut node,
            if excluded_root {
                "CLOUD_EXCLUDED"
            } else {
                "OUTPUT_EXCLUDED"
            },
            "cloud/provider, dataless or output root excluded".into(),
            true,
        );
    }
    let mut count = 1_usize;
    let mut stack = Vec::new();
    if let Some(directory) = directory.filter(|_| !root_excluded) {
        stack.push(Frame { node, directory });
    } else {
        emit(node)?;
    }
    let mut branches=Branches::discover(&root);
    let mut last_progress = Instant::now();
    progress(&Progress {
        branches: branches.rows(),
        entries_seen: count,
        elapsed_ms: 0,
        finished: false,
    });
    while !stack.is_empty() {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        if last_progress.elapsed().as_millis() >= 500 {
            progress(&Progress {
                branches: branches.rows(),
                entries_seen: count,
                elapsed_ms: started.elapsed().as_millis(),
                finished: false,
            });
            last_progress = Instant::now();
        }
        let next = stack.last_mut().unwrap().directory.next_metadata();
        let (name,metadata) = match next {
            Ok(Some(pair)) => pair,
            ended => {
                let mut frame = stack.pop().unwrap();
                if let Err(e) = ended {
                    issue(
                        &mut scan,
                        &mut frame.node,
                        "ENUMERATION_ERROR",
                        e.to_string(),
                        false,
                    );
                }
                frame.node.subtree_end = (count - 1) as u32;
                branches.note(&frame.node);
                emit(frame.node)?;
                continue;
            }
        };
        if count >= options.max_entries || started.elapsed() >= options.max_duration {
            scan.issue(
                0,
                "SCAN_LIMIT",
                "scan",
                "explicit entry or duration limit reached".into(),
                false,
            );
            break;
        }
        let frame = stack.last_mut().unwrap();
        let id = u32::try_from(count).map_err(|_| io::Error::other("entry IDs exhausted"))?;
        count += 1;
        let mut child = StreamEntry {
            id,
            subtree_end: id,
            path: frame.node.path.join(OsStr::from_bytes(name.as_bytes())),
            relative: frame.node.relative.join(OsStr::from_bytes(name.as_bytes())),
            entry: Entry::new(Some(frame.node.id), name.as_bytes().to_vec(), metadata),
        };
        if !metadata.is_dir() {
            branches.note(&child);
            emit(child)?;
            continue;
        }
        if metadata.device != scan.entries[0].device {
            issue(
                &mut scan,
                &mut child,
                "MOUNT_EXCLUDED",
                "different volume excluded".into(),
                true,
            );
        } else if options.exclusions.iter().any(|p| child.path.starts_with(p)) {
            issue(
                &mut scan,
                &mut child,
                "OUTPUT_EXCLUDED",
                "metadata/output path excluded".into(),
                true,
            );
        } else if metadata.dataless() || cloud_path(&child.path) {
            issue(
                &mut scan,
                &mut child,
                "CLOUD_EXCLUDED",
                "cloud/provider or dataless directory excluded".into(),
                true,
            );
        } else if stack.len() >= options.max_depth {
            issue(
                &mut scan,
                &mut child,
                "DEPTH_LIMIT",
                "explicit depth limit reached".into(),
                true,
            );
        } else {
            match stack.last().unwrap().directory.child(&name, metadata) {
                Ok(directory) => {
                    stack.push(Frame {
                        node: child,
                        directory,
                    });
                    continue;
                }
                Err(e) => issue(
                    &mut scan,
                    &mut child,
                    "OPEN_DIRECTORY_ERROR",
                    e.to_string(),
                    false,
                ),
            }
        }
        branches.note(&child);
        emit(child)?;
    }
    while let Some(mut frame) = stack.pop() {
        frame.node.entry.complete = false;
        frame.node.entry.enumeration_complete = false;
        frame.node.subtree_end = (count - 1) as u32;
        emit(frame.node)?;
    }
    scan.context["volume_after"] = capture();
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
            "volume changed during scan".into(),
            false,
        );
    }
    scan.ended_at = platform::timestamp();
    scan.duration_ms = started.elapsed().as_millis();
    progress(&Progress {
        branches: branches.rows(),
        entries_seen: count,
        elapsed_ms: scan.duration_ms,
        finished: true,
    });
    Ok(WalkSummary {
        scan,
        entry_count: count,
    })
}

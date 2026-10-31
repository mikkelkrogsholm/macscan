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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(test)]
thread_local! {static TEST_ELAPSED:std::cell::Cell<Option<Duration>>=const {std::cell::Cell::new(None)};}
fn duration_expired(started:Instant,maximum:Duration)->bool {
    #[cfg(test)] if let Some(elapsed)=TEST_ELAPSED.with(|c|c.get()){return elapsed>=maximum;}
    started.elapsed()>=maximum
}

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
    // Called only after the same root scope gate admitted the traversal.
    fn discover(root:&Path,started:Instant,maximum:Duration,cancel:&AtomicBool)->Self {
        let mut found=Self(Vec::new());
        if duration_expired(started,maximum) || cancel.load(Ordering::Relaxed){return found;}
        if let Ok(mut directory)=Directory::root(root) {
            for _ in 0..4096 {
                if duration_expired(started,maximum) || cancel.load(Ordering::Relaxed){break;}
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
fn issue(scan:&mut Scan,node:&mut StreamEntry,code:&'static str,detail:String,excluded:bool) {
    issue_event(scan,node,code,"scope",detail,excluded,None,None);
}
fn issue_event(scan:&mut Scan,node:&mut StreamEntry,code:&'static str,operation:&'static str,detail:String,excluded:bool,error:Option<&io::Error>,name:Option<&[u8]>) {
    node.entry.complete=false; node.entry.enumeration_complete=false;
    scan.issue_event(0,node.id,code,operation,detail,excluded,error,name);
}

pub fn walk_with_progress(
    requested: &Path,
    options: &ScanOptions,
    cancel: &AtomicBool,
    emit: impl FnMut(StreamEntry) -> io::Result<()>,
    progress: impl FnMut(&Progress),
) -> io::Result<WalkSummary> {
    walk_with_progress_guarded(requested, options, cancel, false, emit, progress)
}

/// Opt-in developer timings over the same guarded production traversal.
/// Forced fallback injects ENOTSUP before each directory's first bulk call,
/// exercising the existing rewind/readdir/fstatat branch without policy changes.
pub fn walk_with_progress_profiled_guarded(
    requested: &Path, options: &ScanOptions, cancel: &AtomicBool,
    require_saved_destination: bool, force_fallback: bool,
    emit: impl FnMut(StreamEntry) -> io::Result<()>, progress: impl FnMut(&Progress),
) -> (io::Result<WalkSummary>, crate::WalkProfile) {
    crate::profile::measure(force_fallback, || walk_with_progress_guarded(
        requested, options, cancel, require_saved_destination, emit, progress))
}

/// Developer-only candidate: one bounded next-batch metadata helper.
pub fn walk_with_progress_prefetch_profiled_guarded(
    requested: &Path, options: &ScanOptions, cancel: &AtomicBool,
    require_saved_destination: bool, force_fallback: bool,
    emit: impl FnMut(StreamEntry) -> io::Result<()>, progress: impl FnMut(&Progress),
) -> (io::Result<WalkSummary>, crate::WalkProfile) {
    crate::profile::measure_prefetch(force_fallback, true, || walk_with_progress_guarded(
        requested, options, cancel, require_saved_destination, emit, progress))
}

/// Focused remeasurement must keep the canonical saved destination. Validation
/// lives after the existing provider/dataless gate, before any root enumeration.
pub fn walk_with_progress_guarded(
    requested: &Path, options: &ScanOptions, cancel: &AtomicBool,
    require_saved_destination: bool,
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
    let saved_destination = requested.clone();
    let root = if excluded_root {
        requested
    } else {
        requested.canonicalize()?
    };
    excluded_root |= cloud_path(&root);
    if require_saved_destination && root != saved_destination {
        return Err(io::Error::new(io::ErrorKind::InvalidInput,
            "Den gemte sti peger nu et andet sted; vælg en ny mappe eksplicit"));
    }
    let root_excluded = options.exclusions.iter().any(|p| root.starts_with(p));
    let directory = if metadata.is_dir() && !excluded_root && !root_excluded {
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
    } else if !excluded_root && !root_excluded {
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
    let mut directory=directory;
    if let Some(d)=&mut directory {d.capture_clones(options.capture_clones);}
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
            "clone_capture":{"version":crate::CLONE_CAPTURE_VERSION,"requested":options.capture_clones,"scope":"admitted_directory_bulk_regular_files","capability_status":"not_queried","regular_file_status_counts":{}},
            "coverage_diagnostics":crate::coverage::empty(),"volume_after":null,"started_unix_ns":nanos.to_string(),"ended_unix_ns":null}),
    };
    let mut node = StreamEntry {
        id: 0,
        subtree_end: 0,
        path: root.clone(),
        relative: PathBuf::new(),
        entry: Entry::new(None, Vec::new(), metadata),
    };
    if excluded_root || root_excluded {
        issue(
            &mut scan,
            &mut node,
            if excluded_root {
                "CLOUD_EXCLUDED"
            } else {
                options.exclusion_code(&root)
            },
            if excluded_root {"cloud/provider or dataless root excluded"}else{"deliberate root path exclusion"}.into(),
            true,
        );
    }
    if metadata.is_file() {
        if options.capture_clones {node.entry.clone_metadata=crate::CloneMetadata::unavailable(crate::CloneStatus::Missing);}
        scan.context["clone_capture"]["regular_file_status_counts"][node.entry.clone_metadata.status.name()]=json!(1);
    }
    let mut count = 1_usize;
    let mut stack = Vec::new();
    if let Some(directory) = directory.filter(|_| !root_excluded) {
        stack.push(Frame { node, directory });
    } else {
        emit(node)?;
    }
    let mut branches=if stack.is_empty(){Branches(Vec::new())}else{Branches::discover(&root,started,options.max_duration,cancel)};
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
        // Failed child metadata reads continue below; enforce the configured
        // budget before every iteration, including those error-only paths.
        if duration_expired(started,options.max_duration) {
            scan.issue(0,"SCAN_LIMIT","scan","explicit duration limit reached".into(),false);
            break;
        }
        if last_progress.elapsed().as_millis() >= 500 {
            progress(&Progress {
                branches: branches.rows(),
                entries_seen: count,
                elapsed_ms: started.elapsed().as_millis(),
                finished: false,
            });
            last_progress = Instant::now();
            // A synchronous progress callback can consume the remaining time.
            // Return through the loop's limit accounting before another read.
            if duration_expired(started,options.max_duration){continue;}
        }
        let next = stack.last_mut().unwrap().directory.next_metadata_event();
        let (name,metadata) = match next {
            Ok(Some(pair)) => pair,
            ended => {
                let mut frame = stack.pop().unwrap();
                if let Err(e) = ended {
                    issue_event(&mut scan,&mut frame.node,"ENUMERATION_ERROR","readdir",e.to_string(),false,Some(&e),None);
                }
                frame.node.subtree_end = (count - 1) as u32;
                branches.note(&frame.node);
                emit(frame.node)?;
                continue;
            }
        };
        let metadata = match metadata {
            Ok(m)=>m,
            Err(e)=>{let frame=stack.last_mut().unwrap();issue_event(&mut scan,&mut frame.node,"METADATA_ERROR","fstatat",e.to_string(),false,Some(&e),Some(name.as_bytes()));continue;}
        };
        if count >= options.max_entries || duration_expired(started,options.max_duration) {
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
        if options.capture_clones && (!metadata.is_file() || metadata.device!=scan.entries[0].device || metadata.dataless() || cloud_path(&child.path) || options.exclusions.iter().any(|p|child.path.starts_with(p))) {
            child.entry.clone_metadata=crate::CloneMetadata::unavailable(crate::CloneStatus::Missing);
        }
        if metadata.is_file() {
            let status=child.entry.clone_metadata.status.name();
            let old=scan.context["clone_capture"]["regular_file_status_counts"][status].as_u64().unwrap_or(0);
            scan.context["clone_capture"]["regular_file_status_counts"][status]=json!(old+1);
        }
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
            let code=options.exclusion_code(&child.path);
            issue(
                &mut scan,
                &mut child,
                code,
                "deliberate path exclusion".into(),
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
                Ok(mut directory) => {
                    directory.capture_clones(options.capture_clones);
                    stack.push(Frame {
                        node: child,
                        directory,
                    });
                    continue;
                }
                Err(e) => issue_event(&mut scan,&mut child,"OPEN_DIRECTORY_ERROR","openat",e.to_string(),false,Some(&e),None),
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

#[cfg(test)]
mod duration_tests {
    use super::*;
    use std::cell::Cell;
    thread_local! {static FAILURES:Cell<usize>=const {Cell::new(0)};}
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_ELAPSED.with(|c|c.set(None));
            platform::METADATA_EVENT_HOOK.with(|c|c.set(None));
        }
    }
    fn failed_child()->io::Result<Option<(CString,io::Result<Metadata>)>> {
        let calls=FAILURES.with(|c|{c.set(c.get()+1);c.get()});
        // Finite even without the fix: the old error-only loop would count
        // three failures and incorrectly finish without a duration event.
        if calls>3{return Ok(None);}
        TEST_ELAPSED.with(|c|c.set(Some(Duration::from_secs(1))));
        Ok(Some((CString::new(format!("synthetic-child-{calls}")).unwrap(),Err(io::Error::from_raw_os_error(libc::EACCES)))))
    }
    fn progress_directory()->io::Result<Option<(CString,io::Result<Metadata>)>> {
        let calls=FAILURES.with(|c|{c.set(c.get()+1);c.get()});
        if calls>3{return Ok(None);}
        TEST_ELAPSED.with(|c|c.set(Some(Duration::from_secs(1))));
        Ok(Some((CString::new(format!("synthetic-branch-{calls}")).unwrap(),Ok(Metadata{mode:libc::S_IFDIR,device:1,inode:1,links:1,logical:0,allocated:0,flags:0,mtime:0,clone_metadata:crate::CloneMetadata::default()}))))
    }
    fn fixture(label:&str)->PathBuf {
        let root=std::env::temp_dir().canonicalize().unwrap().join(format!("macscan-duration-{label}-{}",std::process::id()));
        std::fs::create_dir(&root).unwrap();root
    }
    #[test]
    fn repeated_child_metadata_failures_respect_the_existing_duration_budget() {
        let _reset=Reset;let root=fixture("errors");FAILURES.with(|c|c.set(0));
        TEST_ELAPSED.with(|c|c.set(Some(Duration::ZERO)));
        let options=ScanOptions{max_duration:Duration::from_secs(1),..ScanOptions::default()};
        let mut emitted=Vec::new();
        let result=walk_with_progress(&root,&options,&AtomicBool::new(false),|node|{emitted.push(node);Ok(())},|p|{
            if !p.finished {platform::METADATA_EVENT_HOOK.with(|c|c.set(Some(failed_child)));}
        }).unwrap();
        FAILURES.with(|c|assert_eq!(c.get(),1));
        assert_eq!(result.scan.issue_count,2);
        assert_eq!(result.scan.context["coverage_diagnostics"]["counts"]["access_denied"],1);
        assert_eq!(result.scan.context["coverage_diagnostics"]["counts"]["explicit_limit"],1);
        assert_eq!(result.scan.issues[0].code,"METADATA_ERROR");assert_eq!(result.scan.issues[1].code,"SCAN_LIMIT");
        assert_eq!(result.entry_count,1);assert_eq!(emitted.len(),1);assert!(!emitted[0].entry.enumeration_complete);
        assert_eq!(result.scan.status,ScanStatus::Partial);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn progress_discovery_stops_when_its_first_read_consumes_the_duration() {
        let _reset=Reset;let root=fixture("progress-read");FAILURES.with(|c|c.set(0));
        TEST_ELAPSED.with(|c|c.set(Some(Duration::ZERO)));
        platform::METADATA_EVENT_HOOK.with(|c|c.set(Some(progress_directory)));
        let options=ScanOptions{max_duration:Duration::from_secs(1),..ScanOptions::default()};
        let result=walk_with_progress(&root,&options,&AtomicBool::new(false),|_|Ok(()),|p|assert_eq!(p.branches.len(),1)).unwrap();
        FAILURES.with(|c|assert_eq!(c.get(),1));
        assert_eq!(result.entry_count,1);assert_eq!(result.scan.issue_count,1);
        assert_eq!(result.scan.context["coverage_diagnostics"]["counts"]["explicit_limit"],1);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn expired_duration_skips_progress_discovery_and_traversal_enumeration() {
        let _reset=Reset;let root=fixture("progress");std::fs::create_dir(root.join("branch")).unwrap();
        TEST_ELAPSED.with(|c|c.set(Some(Duration::from_secs(1))));platform::CALLS.with(|c|c.set((0,0)));
        let options=ScanOptions{max_duration:Duration::from_secs(1),..ScanOptions::default()};
        let result=walk_with_progress(&root,&options,&AtomicBool::new(false),|_|Ok(()),|p|assert!(p.branches.is_empty())).unwrap();
        platform::CALLS.with(|c|assert_eq!(c.get(),(1,0))); // Primary root open only; no discovery open/read.
        assert_eq!(result.scan.context["coverage_diagnostics"]["counts"]["explicit_limit"],1);
        assert_eq!(result.entry_count,1);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn experimental_prefetch_is_joined_on_emit_error_stop_and_unwind() {
        let root=fixture("prefetch-lifecycle");
        std::fs::create_dir(root.join("leaf")).unwrap();
        for n in 0..20 {std::fs::write(root.join(format!("leaf/f{n}")),b"x").unwrap();}
        for mode in 0..3 {
            let cancel=AtomicBool::new(false);
            let run=|| walk_with_progress_prefetch_profiled_guarded(&root,&ScanOptions::full(),&cancel,false,false,|node|{
                if node.entry.kind==crate::EntryKind::File {
                    if mode==0 {return Err(io::Error::other("emit failure"));}
                    if mode==1 {cancel.store(true,Ordering::Relaxed);}
                    if mode==2 {panic!("emit unwind");}
                }
                Ok(())
            },|_|{});
            if mode==2 {assert!(std::panic::catch_unwind(run).is_err());}
            else {
                let (result,p)=run();
                assert!(result.is_err());
                assert!(p.prefetch_worker.calls>0); // pending work was drained
            }
            // TLS, worker and descriptors from the previous operation are gone.
            let (result,p)=walk_with_progress_profiled_guarded(&root,&ScanOptions::full(),&AtomicBool::new(false),false,false,|_|Ok(()),|_|{});
            result.unwrap(); assert_eq!(p.prefetch_worker.calls,0);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

}

//! Opt-in, private flat-file scan storage. Descriptor-anchored writes; no target
//! data changes, overwrites, automatic history eviction, or external fallbacks.
use crate::platform::Directory;
use crate::query::invalid;
use crate::scan::hex;
use crate::{Entry, EntryKind, Issue, Scan, ScanStatus, ENGINE_VERSION, SCHEMA_VERSION};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const STORE_BUDGET: u64 = 256 * 1024 * 1024;
const MAX_RECORD: usize = 64 * 1024;
const MAX_ENTRIES: usize = 1_000_000;

pub struct Store {
    path: PathBuf,
    directory: Directory,
}

impl Store {
    /// Parent must exist. Creation is only allowed after explicit --save/UI save.
    pub fn open(path: &Path, create: bool) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(invalid("store path must be absolute"));
        }
        if create {
            match fs::DirBuilder::new().mode(0o700).create(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        let before = path.symlink_metadata()?;
        if !before.is_dir()
            || before.uid() != unsafe { libc::geteuid() }
            || before.mode() & 0o077 != 0
        {
            return Err(invalid("store must be a real, owner-only directory (0700)"));
        }
        let directory = Directory::root(path)?;
        let after = directory.metadata()?;
        if (after.device, after.inode) != (before.dev(), before.ino()) {
            return Err(io::Error::other("store identity changed during open"));
        }
        Ok(Self {
            path: path.to_owned(),
            directory,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn save(&self, scan: &Scan, budget: u64) -> io::Result<()> {
        self.save_with_cancel(scan, budget, &AtomicBool::new(false))
    }

    pub fn save_with_cancel(
        &self,
        scan: &Scan,
        budget: u64,
        cancel: &AtomicBool,
    ) -> io::Result<()> {
        validate_id(&scan.id)?;
        // Only publication is locked, never scanning. Concurrent writers cannot
        // each claim the same free metadata budget. No lock file is created.
        let _lock = StoreLock::acquire(&self.directory)?;
        let used = self.usage()?;
        let remaining = budget
            .checked_sub(used)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::StorageFull,
                    "metadata budget already exhausted",
                )
            })?
            .min(STORE_BUDGET);
        let staging_name = CString::new(format!("{}.partial", scan.id)).unwrap();
        // SAFETY: one validated direct name and an anchored directory descriptor.
        if unsafe { libc::mkdirat(self.directory.fd(), staging_name.as_ptr(), 0o700) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let metadata = self.directory.child_metadata(&staging_name)?;
        let staging = self.directory.child(&staging_name, metadata)?;
        let mut cleanup = Staging {
            parent: &self.directory,
            name: staging_name,
            directory: staging,
            published: false,
        };
        let mut consumed = 0_u64;
        let mut entries = create_file(&cleanup.directory, "entries.jsonl")?;
        let mut buffered = BufWriter::with_capacity(64 * 1024, &mut entries);
        let mut entries_digest = Sha256::new();
        for e in &scan.entries {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "save cancelled"));
            }
            let row = json!([
                e.parent.map(|p| p.to_string()),
                hex(&e.name),
                e.kind.name(),
                e.logical.to_string(),
                e.allocated.to_string(),
                e.accounted.to_string(),
                e.own_allocated.to_string(),
                e.device.to_string(),
                e.inode.to_string(),
                e.links.to_string(),
                e.observed_links,
                e.charged_to.map(|p| p.to_string()),
                e.mtime.to_string(),
                e.complete,
                e.enumeration_complete
            ]);
            entries_digest.update(write_record(&mut buffered, &row, &mut consumed, remaining)?);
            entries_digest.update(b"\n");
        }
        buffered.flush()?;
        drop(buffered);
        entries.sync_all()?;
        // Derived compact preorder index. Only u32 IDs are sorted in RAM;
        // complete paths are materialized one at a time, not retained per file.
        let children = sorted_children(scan, cancel)?;
        let mut index = create_file(&cleanup.directory, "paths.jsonl")?;
        let mut buffer = BufWriter::with_capacity(64 * 1024, &mut index);
        let mut index_digest = Sha256::new();
        let mut id = 0_u32;
        let mut stack: Vec<(usize, usize)> = Vec::new();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "save cancelled"));
            }
            let entry = &scan.entries[id as usize];
            let path = scan.path(id);
            let relative = path
                .strip_prefix(&scan.root)
                .map_err(|_| invalid("entry escaped scan root"))?;
            let row = json!([
                hex(relative.as_os_str().as_bytes()),
                id.to_string(),
                entry.kind.name(),
                entry.complete.then(|| entry.logical.to_string()),
                entry.complete.then(|| entry.accounted.to_string()),
                entry.enumeration_complete
            ]);
            index_digest.update(write_record(&mut buffer, &row, &mut consumed, remaining)?);
            index_digest.update(b"\n");
            if entry.kind == EntryKind::Directory {
                let start = children
                    .partition_point(|child| scan.entries[*child as usize].parent.unwrap() < id);
                let end = children
                    .partition_point(|child| scan.entries[*child as usize].parent.unwrap() <= id);
                stack.push((start, end));
            }
            while stack.last().is_some_and(|(start, end)| start == end) {
                stack.pop();
            }
            let Some((start, _)) = stack.last_mut() else {
                break;
            };
            id = children[*start];
            *start += 1;
        }
        buffer.flush()?;
        drop(buffer);
        index.sync_all()?;
        let manifest = json!({"format_version": "1.2", "schema_version": SCHEMA_VERSION,
            "engine_version": ENGINE_VERSION, "scan_id": scan.id,
            "root_bytes_hex": hex(scan.root.as_os_str().as_bytes()), "policy": scan.policy,
            "context":scan.context,
            "entries_sha256":hex(&entries_digest.finalize()),"paths_sha256":hex(&index_digest.finalize()),
            "status": scan.status.name(), "started_at": scan.started_at, "ended_at": scan.ended_at,
            "duration_ms": scan.duration_ms.to_string(), "entry_count": scan.entries.len(),
            "issue_count": scan.issue_count.to_string(), "excluded_count": scan.excluded_count.to_string(),
            "issues": scan.issues.iter().map(|i| json!({"entry_id": i.entry_id.to_string(), "code": i.code,
                "operation": i.operation, "detail": i.detail, "name_hex": i.name.as_deref().map(hex)})).collect::<Vec<_>>()});
        let mut output = create_file(&cleanup.directory, "manifest.json")?;
        write_record(&mut output, &manifest, &mut consumed, remaining)?;
        output.sync_all()?;
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "save cancelled"));
        }
        // SAFETY: all descriptors are owned/live. fsync returns an OS error on failure.
        if unsafe { libc::fsync(cleanup.directory.fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let final_name = CString::new(scan.id.as_bytes()).unwrap();
        // RENAME_EXCL=4: atomically publish, without replacing even an empty dir.
        // SAFETY: validated single-component names, both relative to same live parent.
        if unsafe {
            libc::renameatx_np(
                self.directory.fd(),
                cleanup.name.as_ptr(),
                self.directory.fd(),
                final_name.as_ptr(),
                4,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        cleanup.published = true;
        if unsafe { libc::fsync(self.directory.fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn load(&self, id: &str) -> io::Result<Scan> {
        validate_id(id)?;
        let name = CString::new(id).unwrap();
        let m = self.directory.child_metadata(&name)?;
        let dir = self.directory.child(&name, m)?;
        let manifest = read_manifest(&dir)?;
        if manifest["format_version"] != "1.2"
            || manifest["schema_version"] != SCHEMA_VERSION
            || manifest["engine_version"] != ENGINE_VERSION
            || manifest["scan_id"] != id
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "incompatible saved scan version/identity",
            ));
        }
        let expected = manifest["entry_count"]
            .as_u64()
            .filter(|n| *n > 0 && *n <= MAX_ENTRIES as u64)
            .ok_or_else(|| invalid("invalid entry count"))? as usize;
        let root = PathBuf::from(std::ffi::OsString::from_vec(unhex(string(
            &manifest["root_bytes_hex"],
        )?)?));
        if !root.is_absolute() {
            return Err(invalid("invalid saved root"));
        }
        let file = read_file(&dir, "entries.jsonl")?;
        let size = file.metadata()?.len();
        if size > STORE_BUDGET {
            return Err(invalid("saved entries exceed metadata budget"));
        }
        let mut input = BufReader::new(VerifiedFile::new(file, &manifest["entries_sha256"])?);
        let mut entries: Vec<Entry> = Vec::new();
        let mut names = 0_usize;
        while let Some(row) = read_record(&mut input)? {
            let array = row
                .as_array()
                .filter(|a| a.len() == 15)
                .ok_or_else(|| invalid("invalid entry row"))?;
            if entries.len() >= expected {
                return Err(invalid("unexpected extra entry"));
            }
            let parent = optional_id(&array[0])?;
            if (entries.is_empty() && parent.is_some())
                || (!entries.is_empty()
                    && parent.is_none_or(|p| {
                        p as usize >= entries.len()
                            || entries[p as usize].kind != EntryKind::Directory
                    }))
            {
                return Err(invalid("invalid parent topology"));
            }
            let name = unhex(string(&array[1])?)?;
            if name.len() > 1024
                || name.contains(&b'/')
                || name.contains(&0)
                || name == b"."
                || name == b".."
                || (!entries.is_empty() && name.is_empty())
            {
                return Err(invalid("invalid raw entry name"));
            }
            names = names.saturating_add(name.len());
            if entries
                .len()
                .saturating_add(1)
                .saturating_mul(192)
                .saturating_add(names)
                > 224 * 1024 * 1024
            {
                return Err(invalid("saved entries exceed memory budget"));
            }
            entries
                .try_reserve(1)
                .map_err(|_| io::Error::other("entry allocation failed"))?;
            let kind = match string(&array[2])? {
                "file" => EntryKind::File,
                "directory" => EntryKind::Directory,
                "symlink" => EntryKind::Symlink,
                "special" => EntryKind::Special,
                _ => return Err(invalid("invalid entry kind")),
            };
            entries.push(Entry {
                parent,
                name,
                kind,
                logical: number(&array[3])?,
                allocated: number(&array[4])?,
                accounted: number(&array[5])?,
                own_allocated: number(&array[6])?,
                device: number(&array[7])?,
                inode: number(&array[8])?,
                links: number(&array[9])?,
                observed_links: array[10]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| invalid("invalid observed links"))?,
                charged_to: optional_id(&array[11])?,
                mtime: string(&array[12])?
                    .parse()
                    .map_err(|_| invalid("invalid mtime"))?,
                complete: array[13]
                    .as_bool()
                    .ok_or_else(|| invalid("invalid coverage"))?,
                enumeration_complete: array[14]
                    .as_bool()
                    .ok_or_else(|| invalid("invalid enumeration coverage"))?,
            });
        }
        if entries.len() != expected {
            return Err(invalid("truncated saved entries"));
        }
        for e in &entries {
            if let Some(target) = e.charged_to {
                let canonical = entries
                    .get(target as usize)
                    .ok_or_else(|| invalid("invalid hardlink target"))?;
                if e.kind != EntryKind::File
                    || (e.device, e.inode) != (canonical.device, canonical.inode)
                    || canonical.charged_to.is_some()
                    || e.accounted != 0
                {
                    return Err(invalid("invalid hardlink accounting"));
                }
            }
        }
        let mut issues = Vec::new();
        for issue in manifest["issues"]
            .as_array()
            .filter(|a| a.len() <= 128)
            .ok_or_else(|| invalid("invalid issues"))?
        {
            let entry_id = u32::try_from(number(&issue["entry_id"])?)
                .map_err(|_| invalid("invalid issue entry"))?;
            if entry_id as usize >= entries.len() {
                return Err(invalid("invalid issue entry"));
            }
            issues.push(Issue {
                entry_id,
                code: string(&issue["code"])?.into(),
                operation: string(&issue["operation"])?.into(),
                detail: string(&issue["detail"])?.into(),
                name: if issue["name_hex"].is_null() {
                    None
                } else {
                    Some(unhex(string(&issue["name_hex"])?)?)
                },
            });
        }
        let status = match string(&manifest["status"])? {
            "complete" if entries[0].complete => ScanStatus::Complete,
            "partial" => ScanStatus::Partial,
            "cancelled" => ScanStatus::Cancelled,
            _ => return Err(invalid("invalid saved status")),
        };
        Ok(Scan {
            id: id.into(),
            root,
            entries,
            issues,
            issue_count: number(&manifest["issue_count"])?,
            excluded_count: number(&manifest["excluded_count"])?,
            status,
            started_at: string(&manifest["started_at"])?.into(),
            ended_at: string(&manifest["ended_at"])?.into(),
            duration_ms: string(&manifest["duration_ms"])?
                .parse()
                .map_err(|_| invalid("invalid duration"))?,
            policy: string(&manifest["policy"])?.into(),
            context: manifest["context"].clone(),
        })
    }

    pub fn ids(&self) -> io::Result<Vec<String>> {
        let mut listing = duplicate_directory(&self.directory)?;
        let mut ids = Vec::new();
        while let Some(name) = listing.next_name()? {
            let Ok(id) = name.to_str() else { continue };
            if validate_id(id).is_ok() && self.directory.child_metadata(&name)?.is_dir() {
                ids.push(id.to_owned());
                if ids.len() > 10_000 {
                    return Err(invalid("store listing exceeds scan count limit"));
                }
            }
        }
        ids.sort();
        Ok(ids)
    }

    pub fn compare(
        &self,
        before_id: &str,
        after_id: &str,
        query: &crate::CompareQuery,
        cancel: &AtomicBool,
    ) -> io::Result<Value> {
        let open = |id: &str| -> io::Result<(Value, BufReader<VerifiedFile>)> {
            validate_id(id)?;
            let name = CString::new(id).unwrap();
            let directory = self
                .directory
                .child(&name, self.directory.child_metadata(&name)?)?;
            let manifest = read_manifest(&directory)?;
            if manifest["format_version"] != "1.2"
                || manifest["scan_id"] != id
                || manifest["schema_version"] != SCHEMA_VERSION
                || manifest["engine_version"] != ENGINE_VERSION
            {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "comparison needs compatible checksummed scans (format 1.2)",
                ));
            }
            let mut entries = VerifiedFile::new(
                read_file(&directory, "entries.jsonl")?,
                &manifest["entries_sha256"],
            )?;
            let mut scratch = [0_u8; 64 * 1024];
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "comparison cancelled",
                    ));
                }
                if entries.read(&mut scratch)? == 0 {
                    break;
                }
            }
            let index = read_file(&directory, "paths.jsonl")?;
            if index.metadata()?.len() > STORE_BUDGET {
                return Err(invalid("comparison index exceeds metadata budget"));
            }
            let checked = VerifiedFile::new(index, &manifest["paths_sha256"])?;
            Ok((manifest, BufReader::new(checked)))
        };
        let (before, left) = open(before_id)?;
        let (after, right) = open(after_id)?;
        crate::compare::compare(&before, &after, left, right, query, cancel)
    }

    fn usage(&self) -> io::Result<u64> {
        let mut total = 0_u64;
        let mut listing = duplicate_directory(&self.directory)?;
        while let Some(name) = listing.next_name()? {
            let metadata = listing.child_metadata(&name)?;
            if !metadata.is_dir() {
                return Err(invalid("unexpected non-directory in private store"));
            }
            let mut child = listing.child(&name, metadata)?;
            while let Some(file) = child.next_name()? {
                let metadata = child.child_metadata(&file)?;
                if !metadata.is_file() {
                    return Err(invalid("unexpected non-file in saved scan"));
                }
                total = total
                    .checked_add(metadata.logical)
                    .ok_or_else(|| invalid("store size overflow"))?;
            }
        }
        Ok(total)
    }
}

// Bottom-up merge sort of compact child IDs, with sticky cancellation checks.
// Comparing (parent ID, basename bytes) avoids reconstructing millions of full
// paths during sorting. Traversal above converts child groups to path preorder.
fn sorted_children(scan: &Scan, cancel: &AtomicBool) -> io::Result<Vec<u32>> {
    if scan.entries.is_empty() {
        return Err(invalid("scan has no root"));
    }
    let mut ids: Vec<u32> = (1..scan.entries.len() as u32).collect();
    for id in &ids {
        let entry = &scan.entries[*id as usize];
        if entry
            .parent
            .is_none_or(|p| p >= *id || scan.entries[p as usize].kind != EntryKind::Directory)
        {
            return Err(invalid("invalid scan topology"));
        }
    }
    let mut scratch = vec![0_u32; ids.len()];
    let mut width = 1_usize;
    while width < ids.len() {
        for start in (0..ids.len()).step_by(width * 2) {
            let middle = (start + width).min(ids.len());
            let end = (start + width * 2).min(ids.len());
            let (mut a, mut b) = (start, middle);
            for out in start..end {
                if out % 1024 == 0 && cancel.load(Ordering::Relaxed) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "index sort cancelled",
                    ));
                }
                let take_a = if b == end {
                    true
                } else if a == middle {
                    false
                } else {
                    let x = &scan.entries[ids[a] as usize];
                    let y = &scan.entries[ids[b] as usize];
                    (x.parent, &x.name) <= (y.parent, &y.name)
                };
                scratch[out] = if take_a {
                    let id = ids[a];
                    a += 1;
                    id
                } else {
                    let id = ids[b];
                    b += 1;
                    id
                };
            }
        }
        std::mem::swap(&mut ids, &mut scratch);
        width = width.saturating_mul(2);
    }
    for pair in ids.windows(2) {
        let x = &scan.entries[pair[0] as usize];
        let y = &scan.entries[pair[1] as usize];
        if (x.parent, &x.name) == (y.parent, &y.name) {
            return Err(invalid(
                "directory changed: duplicate observed path; rescan",
            ));
        }
    }
    Ok(ids)
}

fn duplicate_directory(dir: &Directory) -> io::Result<Directory> {
    let name = CString::new(".").unwrap();
    dir.child(&name, dir.metadata()?)
}

fn validate_id(id: &str) -> io::Result<()> {
    if id.is_empty() || id.len() > 100 || !id.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        return Err(invalid("invalid scan ID"));
    }
    Ok(())
}
fn string(v: &Value) -> io::Result<&str> {
    v.as_str().ok_or_else(|| invalid("expected string"))
}
fn number(v: &Value) -> io::Result<u64> {
    let text = string(v)?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("invalid decimal string"));
    }
    text.parse().map_err(|_| invalid("decimal overflow"))
}
fn optional_id(v: &Value) -> io::Result<Option<u32>> {
    if v.is_null() {
        Ok(None)
    } else {
        u32::try_from(number(v)?)
            .map(Some)
            .map_err(|_| invalid("entry ID overflow"))
    }
}
pub(crate) fn unhex(s: &str) -> io::Result<Vec<u8>> {
    if s.len() > 32768 || s.len() % 2 != 0 {
        return Err(invalid("invalid hex locator length"));
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |b: u8| match b {
                b'0'..=b'9' => Ok(b - b'0'),
                b'a'..=b'f' => Ok(b - b'a' + 10),
                _ => Err(invalid("invalid hex locator")),
            };
            Ok((digit(pair[0])? << 4) | digit(pair[1])?)
        })
        .collect()
}
fn open_file(dir: &Directory, name: &str, write: bool) -> io::Result<File> {
    let name = CString::new(name).unwrap();
    let flags = libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | if write {
            libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY
        } else {
            libc::O_RDONLY | libc::O_NONBLOCK
        };
    // SAFETY: only fixed trusted direct filenames; descriptor anchored to owned dir.
    let fd = unsafe { libc::openat(dir.fd(), name.as_ptr(), flags, 0o600) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: newly owned successful fd, exactly one File owner.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(invalid("metadata file must be a single-link regular file"));
    }
    Ok(file)
}
fn create_file(d: &Directory, n: &str) -> io::Result<File> {
    open_file(d, n, true)
}
fn read_file(d: &Directory, n: &str) -> io::Result<File> {
    open_file(d, n, false)
}

fn write_record(
    file: &mut impl Write,
    row: &Value,
    used: &mut u64,
    remaining: u64,
) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(row)?;
    let next = used
        .checked_add(bytes.len() as u64 + 1)
        .ok_or_else(|| invalid("metadata size overflow"))?;
    if bytes.len() >= MAX_RECORD {
        return Err(invalid("metadata record exceeds size limit"));
    }
    if next > remaining {
        return Err(io::Error::new(
            io::ErrorKind::StorageFull,
            "metadata store budget exhausted",
        ));
    }
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    *used = next;
    Ok(bytes)
}

// Checksums protect against accidental corruption, not a malicious writer who
// can also replace the private manifest. Only saved metadata is hashed; never
// ordinary files under scan targets.
struct VerifiedFile {
    file: File,
    digest: Sha256,
    expected: String,
    read: u64,
}
impl VerifiedFile {
    fn new(file: File, expected: &Value) -> io::Result<Self> {
        let expected = expected
            .as_str()
            .filter(|s| {
                s.len() == 64
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
            .map(str::to_owned)
            .ok_or_else(|| invalid("invalid metadata checksum"))?;
        if file.metadata()?.len() > STORE_BUDGET {
            return Err(invalid("metadata file exceeds bounded size"));
        }
        Ok(Self {
            file,
            digest: Sha256::new(),
            expected,
            read: 0,
        })
    }
}
impl Read for VerifiedFile {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let n = self.file.read(bytes)?;
        self.read = self
            .read
            .checked_add(n as u64)
            .ok_or_else(|| invalid("metadata size overflow"))?;
        if self.read > STORE_BUDGET {
            return Err(invalid("metadata file grew beyond budget"));
        }
        if n == 0 {
            if self.expected != hex(&self.digest.clone().finalize()) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "saved metadata checksum mismatch",
                ));
            }
        } else {
            self.digest.update(&bytes[..n]);
        }
        Ok(n)
    }
}

pub(crate) fn read_record(input: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut bytes = Vec::new();
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(invalid("truncated JSONL record"))
            };
        }
        let end = available.iter().position(|b| *b == b'\n').map(|n| n + 1);
        let take = end.unwrap_or(available.len());
        if bytes.len().saturating_add(take) > MAX_RECORD {
            return Err(invalid("overlarge JSONL record"));
        }
        bytes.extend_from_slice(&available[..take]);
        input.consume(take);
        if end.is_some() {
            return serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| invalid("corrupt JSON record"));
        }
    }
}
fn read_manifest(dir: &Directory) -> io::Result<Value> {
    let mut reader = BufReader::new(read_file(dir, "manifest.json")?);
    let row = read_record(&mut reader)?.ok_or_else(|| invalid("empty manifest"))?;
    if !reader.fill_buf()?.is_empty() {
        return Err(invalid("unexpected manifest trailer"));
    }
    Ok(row)
}

struct StoreLock<'a>(&'a Directory);
impl<'a> StoreLock<'a> {
    fn acquire(dir: &'a Directory) -> io::Result<Self> {
        if unsafe { libc::flock(dir.fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(dir))
    }
}
impl Drop for StoreLock<'_> {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.fd(), libc::LOCK_UN);
        }
    }
}

struct Staging<'a> {
    parent: &'a Directory,
    name: CString,
    directory: Directory,
    published: bool,
}
impl Drop for Staging<'_> {
    fn drop(&mut self) {
        if !self.published {
            // Only the exact staging directory created by this invocation. No
            // traversal, glob/prefix deletion or retained scan is touched.
            for filename in [c"entries.jsonl", c"paths.jsonl", c"manifest.json"] {
                unsafe {
                    libc::unlinkat(self.directory.fd(), filename.as_ptr(), 0);
                }
            }
            unsafe {
                libc::unlinkat(self.parent.fd(), self.name.as_ptr(), libc::AT_REMOVEDIR);
            }
        }
    }
}

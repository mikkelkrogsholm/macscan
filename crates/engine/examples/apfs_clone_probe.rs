//! Opt-in synthetic metadata experiment; never called by normal scanning.
//! See docs/apfs-clone-findings.md. No third-party implementation copied.
use serde_json::json;
use std::{
    collections::HashSet,
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{FileExt, MetadataExt},
        },
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop_requested(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}
fn check_stop() -> io::Result<()> {
    if STOP.load(Ordering::Relaxed) {
        Err(io::Error::new(io::ErrorKind::Interrupted, "probe stopped"))
    } else {
        Ok(())
    }
}
const CLONE_ID: u32 = 0x100;
const EXT_FLAGS: u32 = 0x200;
const DATA_ALLOC: u32 = 0x400;
const EXTENDED: u64 = 0x20;
const SHARES_ALL: u64 = 0x40;
const COMMON: u32 = libc::ATTR_CMN_RETURNED_ATTRS
    | libc::ATTR_CMN_NAME
    | libc::ATTR_CMN_DEVID
    | libc::ATTR_CMN_FILEID;
const FILE_ATTR: u32 = libc::ATTR_FILE_ALLOCSIZE | DATA_ALLOC;
#[derive(Clone, Debug)]
struct Row {
    name: String,
    device: Option<u32>,
    inode: Option<u64>,
    total: Option<u64>,
    data: Option<u64>,
    clone_id: Option<u64>,
    flags: Option<u64>,
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid probe attribute record")
}
fn u32_at(b: &[u8], p: usize) -> io::Result<u32> {
    Ok(u32::from_ne_bytes(
        b.get(p..p + 4).ok_or_else(invalid)?.try_into().unwrap(),
    ))
}
fn u64_at(b: &[u8], p: usize) -> io::Result<u64> {
    Ok(u64::from_ne_bytes(
        b.get(p..p + 8).ok_or_else(invalid)?.try_into().unwrap(),
    ))
}
fn parse(b: &[u8], extended: bool) -> io::Result<Row> {
    if u32_at(b, 0)? as usize != b.len() || b.len() < 24 {
        return Err(invalid());
    }
    let c = u32_at(b, 4)?;
    let v = u32_at(b, 8)?;
    let d = u32_at(b, 12)?;
    let f = u32_at(b, 16)?;
    let x = u32_at(b, 20)?;
    if c & !COMMON != 0
        || v != 0
        || d != 0
        || f & !FILE_ATTR != 0
        || x & !(if extended { CLONE_ID | EXT_FLAGS } else { 0 }) != 0
        || c & libc::ATTR_CMN_NAME == 0
        || c & libc::ATTR_CMN_RETURNED_ATTRS == 0
    {
        return Err(invalid());
    }
    let mut p = 24;
    let delta = u32_at(b, p)? as i32;
    let len = u32_at(b, p + 4)? as usize;
    let start = (p as i64)
        .checked_add(delta as i64)
        .filter(|n| *n >= 0)
        .ok_or_else(invalid)? as usize;
    let end = start.checked_add(len).ok_or_else(invalid)?;
    let bytes = b
        .get(start..end)
        .filter(|s| s.len() > 1 && s.last() == Some(&0))
        .ok_or_else(invalid)?;
    if bytes[..bytes.len() - 1].contains(&0) || bytes.contains(&b'/') {
        return Err(invalid());
    }
    let name = String::from_utf8(bytes[..bytes.len() - 1].to_vec()).map_err(|_| invalid())?;
    p += 8;
    let device = if c & libc::ATTR_CMN_DEVID != 0 {
        let n = u32_at(b, p)?;
        p += 4;
        Some(n)
    } else {
        None
    };
    let inode = if c & libc::ATTR_CMN_FILEID != 0 {
        let n = u64_at(b, p)?;
        p += 8;
        Some(n)
    } else {
        None
    };
    let mut take = |present: bool| -> io::Result<Option<u64>> {
        if present {
            let n = u64_at(b, p)?;
            p += 8;
            Ok(Some(n))
        } else {
            Ok(None)
        }
    };
    let total = take(f & libc::ATTR_FILE_ALLOCSIZE != 0)?;
    let data = take(f & DATA_ALLOC != 0)?;
    let clone_id = take(x & CLONE_ID != 0)?.filter(|n| *n != 0);
    let flags = take(x & EXT_FLAGS != 0)?;
    if start < p
        || total.is_some_and(|n| n > i64::MAX as u64)
        || data.is_some_and(|n| n > i64::MAX as u64)
        || data.zip(total).is_some_and(|(data, total)| data > total)
    {
        return Err(invalid());
    }
    Ok(Row {
        name,
        device,
        inode,
        total,
        data,
        clone_id,
        flags,
    })
}
fn enumerate(root: &Path, extended: bool) -> io::Result<Vec<Row>> {
    let fd = File::open(root)?;
    let mut attrs = libc::attrlist {
        bitmapcount: 5,
        reserved: 0,
        commonattr: COMMON,
        volattr: 0,
        dirattr: 0,
        fileattr: FILE_ATTR,
        forkattr: if extended { CLONE_ID | EXT_FLAGS } else { 0 },
    };
    let mut b = vec![0u8; 64 * 1024];
    let mut rows = Vec::new();
    loop {
        check_stop()?;
        // SAFETY: live directory descriptor, public attrlist and bounded writable buffer.
        let n = unsafe {
            libc::getattrlistbulk(
                fd.as_raw_fd(),
                (&mut attrs as *mut libc::attrlist).cast(),
                b.as_mut_ptr().cast(),
                b.len(),
                if extended { EXTENDED } else { 0 },
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        if n == 0 {
            break;
        }
        let mut p = 0;
        for _ in 0..n {
            let len = u32_at(&b, p)? as usize;
            if len < 24 {
                return Err(invalid());
            }
            let end = p.checked_add(len).ok_or_else(invalid)?;
            rows.push(parse(b.get(p..end).ok_or_else(invalid)?, extended)?);
            p = end;
        }
    }
    Ok(rows)
}
// This is a diagnostic fallback, never a byte-copy fallback for clonefile.
fn optional_clone_metadata(
    mut request: impl FnMut(bool) -> io::Result<Vec<Row>>,
) -> io::Result<(Vec<Row>, bool)> {
    match request(true) {
        Ok(rows) => Ok((rows, true)),
        Err(e) if matches!(e.raw_os_error(), Some(libc::EINVAL) | Some(libc::ENOTSUP)) => {
            Ok((request(false)?, false))
        }
        Err(e) => Err(e),
    }
}
// Experimental supplementary observation only. Raw and hardlink sums remain separate.
fn account(rows: &[Row]) -> io::Result<(u64, u64, u64)> {
    let (mut raw, mut hardlink, mut adjusted) = (0, 0, 0);
    let mut inodes = HashSet::new();
    let mut clones = HashSet::new();
    for r in rows {
        let total = r.total.ok_or_else(invalid)?;
        raw += total;
        if let Some(key) = r.device.zip(r.inode) {
            if !inodes.insert(key) {
                continue;
            }
        }
        hardlink += total;
        adjusted += total;
        if r.flags.is_some_and(|f| f & SHARES_ALL != 0) {
            if let Some(((dev, id), data)) = r.device.zip(r.clone_id).zip(r.data) {
                if data > total {
                    return Err(invalid());
                }
                if !clones.insert((dev, id)) {
                    adjusted -= data;
                }
            }
        }
    }
    Ok((raw, hardlink, adjusted))
}
struct Tree(PathBuf);
impl Drop for Tree {
    fn drop(&mut self) {
        match fs::remove_dir_all(&self.0) {
            Ok(()) => eprintln!("private fixture cleanup complete"),
            Err(e) => eprintln!("fixture cleanup failed: {e}"),
        }
    }
}
fn tree() -> io::Result<Tree> {
    let mut template = std::env::temp_dir()
        .join("macscan-apfs-probe-XXXXXX")
        .as_os_str()
        .as_bytes()
        .to_vec();
    template.push(0);
    // SAFETY: writable nul-terminated mkdtemp template; returned private directory is owned here.
    if unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) }.is_null() {
        return Err(io::Error::last_os_error());
    }
    template.pop();
    Ok(Tree(PathBuf::from(std::ffi::OsStr::from_bytes(&template))))
}
fn clone_file(a: &Path, b: &Path) -> io::Result<()> {
    let a = CString::new(a.as_os_str().as_bytes()).map_err(|_| invalid())?;
    let b = CString::new(b.as_os_str().as_bytes()).map_err(|_| invalid())?;
    // SAFETY: valid nul-terminated paths inside the owned fixture; no copy fallback.
    if unsafe { libc::clonefile(a.as_ptr(), b.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
fn capabilities(root: &Path) -> io::Result<serde_json::Value> {
    let path = CString::new(root.as_os_str().as_bytes()).map_err(|_| invalid())?;
    let mut attrs = libc::attrlist {
        bitmapcount: 5,
        reserved: 0,
        commonattr: libc::ATTR_CMN_RETURNED_ATTRS,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_CAPABILITIES,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut b = [0u8; 256];
    // SAFETY: initialized public request, bounded output, valid fixture path.
    if unsafe {
        libc::getattrlist(
            path.as_ptr(),
            (&mut attrs as *mut libc::attrlist).cast(),
            b.as_mut_ptr().cast(),
            b.len(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if u32_at(&b, 8)? & libc::ATTR_VOL_CAPABILITIES == 0 || u32_at(&b, 0)? < 56 {
        return Err(invalid());
    }
    let format = u32_at(&b, 24)?;
    let interface = u32_at(&b, 28)?;
    let valid_format = u32_at(&b, 40)?;
    let valid_interface = u32_at(&b, 44)?;
    Ok(
        json!({"clone_mapping_valid":valid_format & 0x04000000 != 0,"clone_mapping":format & 0x04000000 != 0,"clone_interface_valid":valid_interface & 0x10000 != 0,"clone_interface":interface & 0x10000 != 0}),
    )
}
fn run() -> io::Result<()> {
    if std::env::var_os("MACSCAN_APFS_SYNTHETIC_PROBE").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return Err(io::Error::other(
            "explicit opt-in required: MACSCAN_APFS_SYNTHETIC_PROBE=1",
        ));
    }
    // SAFETY: handler only stores a lock-free atomic flag; cleanup occurs in normal control flow.
    unsafe {
        libc::signal(
            libc::SIGINT,
            stop_requested as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            stop_requested as *const () as libc::sighandler_t,
        );
    }
    let tree = tree()?;
    let root = &tree.0;
    eprintln!("created private fixture: {}", root.display());
    let source = root.join("source");
    let mut source_file = File::create(&source)?;
    source_file.write_all(&vec![0x5a; 1024 * 1024])?;
    source_file.sync_all()?;
    for name in ["full", "forked", "partial"] {
        clone_file(&source, &root.join(name))?;
    }
    fs::hard_link(&source, root.join("hardlink"))?;
    let partial = OpenOptions::new().write(true).open(root.join("partial"))?;
    partial.write_all_at(&[0x31; 4096], 0)?;
    partial.sync_all()?;
    let mut fork = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(root.join("forked/..namedfork/rsrc"))?;
    fork.write_all(&vec![0x73; 65536])?;
    fork.sync_all()?;
    let sparse = File::create(root.join("sparse"))?;
    sparse.set_len(16 * 1024 * 1024)?;
    sparse.write_all_at(&[0x22; 4096], 16 * 1024 * 1024 - 4096)?;
    sparse.sync_all()?;
    let hole = libc::fpunchhole_t {
        fp_flags: 0,
        reserved: 0,
        fp_offset: 0,
        fp_length: 16 * 1024 * 1024 - 4096,
    };
    // SAFETY: live owned file, correctly initialized public F_PUNCHHOLE input.
    if unsafe { libc::fcntl(sparse.as_raw_fd(), libc::F_PUNCHHOLE, &hole) } != 0 {
        return Err(io::Error::last_os_error());
    }
    sparse.sync_all()?;
    let caps = capabilities(root)?;
    let (mut rows, clone_attributes_supported) =
        optional_clone_metadata(|extended| enumerate(root, extended))?;
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    let mut output = Vec::new();
    for r in &rows {
        let stat = fs::metadata(root.join(&r.name))?;
        output.push(json!({"name":r.name,"device":r.device,"inode":r.inode,"clone_id":r.clone_id,"extended_flags":r.flags,"shares_all":r.flags.map(|f|f&SHARES_ALL!=0),"total_allocated":r.total,"data_allocated":r.data,"stat_allocated":stat.blocks()*512,"logical":stat.len()}));
    }
    let get = |name: &str| rows.iter().find(|r| r.name == name).ok_or_else(invalid);
    let source_row = get("source")?;
    let full = get("full")?;
    let forked = get("forked")?;
    let partial = get("partial")?;
    let hard = get("hardlink")?;
    let sparse_row = get("sparse")?;
    if sparse_row.data != Some(4096) || sparse_row.total != Some(4096) {
        return Err(io::Error::other(
            "punched sparse allocation invariant failed",
        ));
    }
    if source_row.inode == full.inode || source_row.inode != hard.inode {
        return Err(io::Error::other("clone/hardlink inode invariant failed"));
    }
    if clone_attributes_supported {
        let id = source_row
            .clone_id
            .ok_or_else(|| io::Error::other("clone ID absent on synthetic source"))?;
        if full.clone_id != Some(id)
            || forked.clone_id != Some(id)
            || ![source_row, full, forked]
                .iter()
                .all(|r| r.flags.is_some_and(|f| f & SHARES_ALL != 0))
            || partial.flags.is_some_and(|f| f & SHARES_ALL != 0)
            || partial.clone_id == Some(id)
        {
            return Err(io::Error::other(
                "synthetic full/partial clone metadata invariant failed",
            ));
        }
    }
    if forked.total.zip(forked.data).is_none_or(|(t, d)| t <= d) {
        return Err(io::Error::other(
            "resource fork allocation not independently observed",
        ));
    }
    let (raw, hardlink, adjusted) = account(&rows)?;
    for i in 0..994 {
        check_stop()?;
        File::create(root.join(format!("tiny-{i:04}")))?.write_all(&[i as u8; 128])?;
    }
    let mut timings = Vec::new();
    // Warm both requests, then alternate order to reduce order bias. Same data-fork request in both.
    assert_eq!(enumerate(root, false)?.len(), 1000);
    if clone_attributes_supported {
        assert_eq!(enumerate(root, true)?.len(), 1000);
    }
    for trial in 0..12 {
        for extended in if trial % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            if extended && !clone_attributes_supported {
                continue;
            }
            let start = Instant::now();
            let count = enumerate(root, extended)?.len();
            timings.push(json!({"trial":trial,"extended":extended,"count":count,"elapsed_ns":start.elapsed().as_nanos()}));
        }
    }
    println!(
        "{}",
        json!({"clone_attributes_supported":clone_attributes_supported,"capabilities":caps,"fixtures":output,"raw":raw,"hardlink_accounted":hardlink,"experimental_clone_adjusted":adjusted,"timings":timings})
    );
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("probe failed: {e}");
        std::process::exit(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn row(dev: u32, inode: u64, id: Option<u64>, flags: Option<u64>) -> Row {
        Row {
            name: String::new(),
            device: Some(dev),
            inode: Some(inode),
            total: Some(120),
            data: Some(100),
            clone_id: id,
            flags,
        }
    }
    fn record(x: u32, values: &[u64]) -> Vec<u8> {
        let mut b = vec![0u8; 32];
        b[4..8]
            .copy_from_slice(&(libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_NAME).to_ne_bytes());
        b[20..24].copy_from_slice(&x.to_ne_bytes());
        for n in values {
            b.extend(n.to_ne_bytes());
        }
        let start = b.len();
        b.extend(b"a\0");
        let len = b.len() as u32;
        b[..4].copy_from_slice(&len.to_ne_bytes());
        b[24..28].copy_from_slice(&((start - 24) as u32).to_ne_bytes());
        b[28..32].copy_from_slice(&2u32.to_ne_bytes());
        b
    }
    #[test]
    fn full_data_clones_keep_independent_non_data_and_hardlinks_once() {
        assert_eq!(
            account(&[
                row(1, 1, Some(7), Some(64)),
                row(1, 2, Some(7), Some(64)),
                row(1, 1, Some(7), Some(64))
            ])
            .unwrap(),
            (360, 240, 140)
        );
    }
    #[test]
    fn volume_identity_prevents_cross_device_dedup() {
        assert_eq!(
            account(&[row(1, 1, Some(7), Some(64)), row(2, 1, Some(7), Some(64))]).unwrap(),
            (240, 240, 240)
        );
    }
    #[test]
    fn partial_missing_zero_or_unknown_never_dedup() {
        for (id, flags) in [(Some(7), Some(1)), (None, Some(64)), (Some(7), None)] {
            assert_eq!(
                account(&[row(1, 1, id, flags), row(1, 2, id, flags)])
                    .unwrap()
                    .2,
                240
            );
        }
        let r = parse(&record(CLONE_ID | EXT_FLAGS, &[0, 64]), true).unwrap();
        assert!(r.clone_id.is_none());
    }
    #[test]
    fn returned_bits_control_optional_layout() {
        let r = parse(&record(EXT_FLAGS, &[64]), true).unwrap();
        assert_eq!(r.flags, Some(64));
        assert!(r.clone_id.is_none());
        let r = parse(&record(0, &[]), true).unwrap();
        assert!(r.flags.is_none());
        assert!(r.clone_id.is_none());
    }
    #[test]
    fn malformed_truncated_and_unrequested_fields_rejected() {
        let b = record(CLONE_ID, &[1]);
        for end in 0..b.len() {
            assert!(parse(&b[..end], true).is_err());
        }
        assert!(parse(&record(4, &[1]), true).is_err());
        assert!(parse(&record(CLONE_ID, &[1]), false).is_err());
        let mut b = record(0, &[]);
        b[24..28].copy_from_slice(&i32::MAX.to_ne_bytes());
        assert!(parse(&b, true).is_err());
    }
    #[test]
    fn missing_data_or_device_retains_total() {
        let a = row(1, 1, Some(7), Some(64));
        let mut b = row(1, 2, Some(7), Some(64));
        b.data = None;
        assert_eq!(account(&[a.clone(), b]).unwrap().2, 240);
        let mut b = row(1, 2, Some(7), Some(64));
        b.device = None;
        assert_eq!(account(&[a, b]).unwrap().2, 240);
    }
    #[test]
    fn unsupported_syscall_retries_baseline_without_grouping() {
        for error in [libc::EINVAL, libc::ENOTSUP] {
            let mut calls = Vec::new();
            let (rows, supported) = optional_clone_metadata(|extended| {
                calls.push(extended);
                if extended {
                    Err(io::Error::from_raw_os_error(error))
                } else {
                    Ok(vec![row(1, 1, None, None), row(1, 2, None, None)])
                }
            })
            .unwrap();
            assert!(!supported);
            assert_eq!(calls, [true, false]);
            assert_eq!(account(&rows).unwrap().2, 240);
        }
        let mut calls = 0;
        assert!(optional_clone_metadata(|_| {
            calls += 1;
            Err(io::Error::from_raw_os_error(libc::EACCES))
        })
        .is_err());
        assert_eq!(calls, 1);
    }
    #[test]
    fn negative_allocation_is_rejected() {
        let mut b = record(0, &[u64::MAX]);
        b[16..20].copy_from_slice(&DATA_ALLOC.to_ne_bytes());
        assert!(parse(&b, true).is_err());
    }
    #[test]
    fn data_larger_than_total_rejected() {
        let mut a = row(1, 1, Some(7), Some(64));
        a.data = Some(121);
        assert!(account(&[a]).is_err());
    }
}

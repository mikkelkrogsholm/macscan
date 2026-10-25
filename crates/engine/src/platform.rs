//! Darwin's public descriptor-relative APIs. All traversal stays anchored to
//! already-open directories; no ordinary file is opened for reading contents.
use std::ffi::CString;
use std::io;
use std::mem::MaybeUninit;
use std::os::macos::fs::MetadataExt as MacMetadataExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Metadata {
    pub mode: u16,
    pub device: u64,
    pub inode: u64,
    pub links: u64,
    pub logical: u64,
    pub allocated: u64,
    pub flags: u32,
    pub mtime: i64,
}

impl Metadata {
    pub fn from_std(s: &std::fs::Metadata) -> io::Result<Self> {
        Ok(Self {
            mode: s.mode() as u16,
            device: s.dev(),
            inode: s.ino(),
            links: s.nlink(),
            logical: s.size(),
            allocated: s
                .blocks()
                .checked_mul(512)
                .ok_or_else(|| io::Error::other("allocated size overflow"))?,
            flags: s.st_flags(),
            mtime: s.mtime(),
        })
    }
    fn from_stat(s: &libc::stat) -> io::Result<Self> {
        let logical =
            u64::try_from(s.st_size).map_err(|_| io::Error::other("negative file size"))?;
        let allocated = u64::try_from(s.st_blocks)
            .ok()
            .and_then(|b| b.checked_mul(512))
            .ok_or_else(|| io::Error::other("invalid allocated size"))?;
        Ok(Self {
            mode: s.st_mode,
            device: s.st_dev as u32 as u64,
            inode: s.st_ino,
            links: s.st_nlink as u64,
            logical,
            allocated,
            flags: s.st_flags,
            mtime: s.st_mtime,
        })
    }

    pub fn is_dir(self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFDIR
    }
    pub fn is_file(self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFREG
    }
    pub fn is_symlink(self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFLNK
    }
    pub fn dataless(self) -> bool {
        self.flags & 0x4000_0000 != 0
    } // SF_DATALESS, sys/stat.h
}

pub(crate) struct Directory(*mut libc::DIR, Option<crate::bulk::Bulk>);

impl Directory {
    pub fn root(path: &Path) -> io::Result<Self> {
        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in root"))?;
        // O_NOFOLLOW_ANY is public in the minimum supported Mac SDK.
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | 0x2000_0000;
        // SAFETY: NUL-terminated path, no variadic mode needed for read-only open.
        let fd = unsafe { libc::open(path.as_ptr(), flags) };
        Self::from_fd(fd)
    }

    fn from_fd(fd: libc::c_int) -> io::Result<Self> {
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd is a newly owned directory descriptor. fdopendir takes ownership on success.
        let dir = unsafe { libc::fdopendir(fd) };
        if dir.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: ownership was not transferred on failure.
            unsafe {
                libc::close(fd);
            }
            return Err(error);
        }
        Ok(Self(dir, Some(crate::bulk::Bulk::new())))
    }

    pub(crate) fn fd(&self) -> libc::c_int {
        // SAFETY: self owns a live DIR for this method's duration.
        unsafe { libc::dirfd(self.0) }
    }

    pub fn metadata(&self) -> io::Result<Metadata> {
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fstat writes one stat when it succeeds; fd is live.
        if unsafe { libc::fstat(self.fd(), stat.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful fstat initialized the struct.
        Metadata::from_stat(unsafe { &stat.assume_init() })
    }

    pub fn child_metadata(&self, name: &CString) -> io::Result<Metadata> {
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: name is a direct directory entry, fd is live, output is writable.
        if unsafe {
            libc::fstatat(
                self.fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful fstatat initialized the struct.
        Metadata::from_stat(unsafe { &stat.assume_init() })
    }

    pub fn child(&self, name: &CString, expected: Metadata) -> io::Result<Self> {
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        // SAFETY: one direct entry, never a path with slash; no symlink traversal.
        let fd = unsafe { libc::openat(self.fd(), name.as_ptr(), flags) };
        let child = Self::from_fd(fd)?;
        let observed = child.metadata()?;
        if (observed.device, observed.inode) != (expected.device, expected.inode) {
            return Err(io::Error::other("directory identity changed during open"));
        }
        Ok(child)
    }

    pub fn next_name(&mut self) -> io::Result<Option<CString>> {
        loop {
            // SAFETY: errno is thread local; this Directory is used on one thread.
            unsafe {
                *libc::__error() = 0;
            }
            // SAFETY: a live DIR; returned memory remains valid until next readdir call.
            let entry = unsafe { libc::readdir(self.0) };
            if entry.is_null() {
                let error = io::Error::last_os_error();
                return if error.raw_os_error() == Some(0) {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
            // SAFETY: non-null readdir result, borrowed before next iteration.
            let entry = unsafe { &*entry };
            let length = entry.d_namlen as usize;
            if length > entry.d_name.len() {
                return Err(io::Error::other("invalid directory name length"));
            }
            let bytes: Vec<u8> = entry.d_name[..length].iter().map(|&c| c as u8).collect();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if bytes.contains(&b'/') {
                return Err(io::Error::other("slash in directory entry"));
            }
            return CString::new(bytes)
                .map(Some)
                .map_err(|_| io::Error::other("embedded NUL in directory entry"));
        }
    }

    pub fn next_metadata(&mut self) -> io::Result<Option<(CString, Metadata)>> {
        if let Some(bulk)=&mut self.1 {
            match bulk.next(unsafe {libc::dirfd(self.0)}) {
                Ok(Some((name,metadata)))=>return Ok(Some((name.clone(),match metadata {Some(m)=>m,None=>self.child_metadata(&name)?}))),
                Ok(None)=>return Ok(None),
                Err(e) if !bulk.used && [Some(libc::EINVAL),Some(libc::ENOTSUP),Some(libc::ENOSYS)].contains(&e.raw_os_error())=>{
                    // No rows consumed. Never mix readdir and bulk on an advanced cursor.
                    unsafe {libc::rewinddir(self.0)};
                    self.1=None;
                },
                Err(e)=>return Err(e),
            }
        }
        self.next_name()?.map(|n|self.child_metadata(&n).map(|m|(n,m))).transpose()
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        // SAFETY: unique DIR owner, exactly one close.
        unsafe {
            libc::closedir(self.0);
        }
    }
}

pub(crate) fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    let mut utc = MaybeUninit::<libc::tm>::uninit();
    // SAFETY: now and output storage are valid; gmtime_r is reentrant.
    if unsafe { libc::gmtime_r(&now, utc.as_mut_ptr()) }.is_null() {
        return "unavailable".into();
    }
    // SAFETY: successful gmtime_r initialized utc.
    let utc = unsafe { utc.assume_init() };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        utc.tm_year + 1900,
        utc.tm_mon + 1,
        utc.tm_mday,
        utc.tm_hour,
        utc.tm_min,
        utc.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utc_shape() {
        let s = timestamp();
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z'));
    }
    #[test]
    fn bulk_matches_stat_for_regular_sparse_hardlinked_and_hostile_names(){
        use std::os::unix::fs::symlink;
        let root=std::env::temp_dir().canonicalize().unwrap().join(format!("macscan-bulk-parity-{}",std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a"),b"abc").unwrap();
        std::fs::hard_link(root.join("a"),root.join("b")).unwrap();
        std::fs::File::create(root.join("sparse")).unwrap().set_len(100_000_000).unwrap();
        std::fs::write(root.join("æ\"\nname"),b"z").unwrap();
        std::fs::create_dir(root.join("directory")).unwrap();symlink("a",root.join("link")).unwrap();
        let mut d=Directory::root(&root).unwrap();let mut count=0;
        while let Some((name,m))=d.next_metadata().unwrap(){let expected=d.child_metadata(&name).unwrap();
            assert_eq!((m.mode&libc::S_IFMT,m.device,m.inode,m.links,m.logical,m.allocated,m.flags,m.mtime),(expected.mode&libc::S_IFMT,expected.device,expected.inode,expected.links,expected.logical,expected.allocated,expected.flags,expected.mtime));count+=1;
        }
        assert_eq!(count,6);assert!(d.1.as_ref().unwrap().used);drop(d);std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore="Creates 10,000 disposable empty files to compare metadata enumeration"]
    fn bulk_benchmark(){
        let root=std::env::temp_dir().canonicalize().unwrap().join(format!("macscan-bulk-bench-{}",std::process::id()));std::fs::create_dir(&root).unwrap();
        for i in 0..10_000{std::fs::File::create(root.join(format!("f{i}"))).unwrap();}
        for round in 0..3 {
            let start=std::time::Instant::now();let mut d=Directory::root(&root).unwrap();let mut count=0;
            while let Some(name)=d.next_name().unwrap(){d.child_metadata(&name).unwrap();count+=1;}
            let stat=start.elapsed();drop(d);let start=std::time::Instant::now();let mut d=Directory::root(&root).unwrap();let mut bulk_count=0;
            while d.next_metadata().unwrap().is_some(){bulk_count+=1;}
            assert_eq!(count,bulk_count);assert_eq!(count,10_000);println!("round={round} stat_us={} bulk_us={}",stat.as_micros(),start.elapsed().as_micros());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

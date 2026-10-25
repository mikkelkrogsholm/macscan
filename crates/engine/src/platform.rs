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

pub(crate) struct Directory(*mut libc::DIR);

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
        Ok(Self(dir))
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
}

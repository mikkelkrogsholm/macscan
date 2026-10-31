//! Bounded Darwin metadata batches. Unavailable attributes fall back to fstatat.
use crate::platform::Metadata;
use std::{collections::VecDeque, ffi::CString, io};

const COMMON: u32 = libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_NAME
    | libc::ATTR_CMN_DEVID | libc::ATTR_CMN_OBJTYPE | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_FLAGS | libc::ATTR_CMN_FILEID;
const FILE: u32 = libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_ALLOCSIZE | libc::ATTR_FILE_DATALENGTH;
pub(crate) struct Bulk {
    buffer: Vec<u8>,
    rows: VecDeque<(CString, Option<Metadata>)>,
    pub used: bool,
    pending: Option<crate::prefetch::Pending>,
}
impl Bulk {
    pub fn new() -> Self { Self { buffer: vec![0; 64*1024], rows: VecDeque::new(), used:false, pending:None } }
    pub fn next(&mut self, fd:i32) -> io::Result<Option<(CString,Option<Metadata>)>> {
        if let Some(row)=self.rows.pop_front() {return Ok(Some(row));}
        if !self.used && crate::profile::force_fallback() {
            return Err(io::Error::from_raw_os_error(libc::ENOTSUP));
        }
        let n = if let Some(pending) = self.pending.take() {
            let (buffer, result) = pending.finish();
            self.buffer = buffer;
            result?
        } else {
            crate::profile::timed(crate::profile::Call::Bulk, || read_batch(fd, &mut self.buffer))?
        };
        self.used=true;
        let mut offset=0;
        for _ in 0..n {
            let length=word(&self.buffer,offset)? as usize;
            if length<24 || length>self.buffer.len()-offset {return Err(invalid());}
            let record=&self.buffer[offset..offset+length];
            self.rows.push_back(parse(record)?);
            offset+=length;
        }
        // Do not reserve the global slot across a directory descent. Batches
        // containing nonregular/missing metadata stay serial; all-file batches
        // can overlap row processing/Parquet callbacks with their next batch.
        if n > 0 && self.rows.iter().all(|(_, metadata)| metadata.is_some()) {
            self.pending = crate::profile::prefetch(fd, &mut self.buffer);
        }
        Ok(self.rows.pop_front())
    }
}
// Both coordinator and experimental helper use this exact public Darwin call.
pub(crate) fn read_batch(fd: i32, buffer: &mut [u8]) -> io::Result<usize> {
    let mut attrs=libc::attrlist {bitmapcount:5,reserved:0,commonattr:COMMON,volattr:0,dirattr:0,fileattr:FILE,forkattr:0};
    // SAFETY: caller owns a live directory fd and a bounded writable buffer.
    let n=unsafe {libc::getattrlistbulk(fd,(&mut attrs as *mut libc::attrlist).cast(),buffer.as_mut_ptr().cast(),buffer.len(),0)};
    if n<0 {Err(io::Error::last_os_error())} else {Ok(n as usize)}
}
impl Drop for Bulk {
    fn drop(&mut self) {
        // A stopped/erroring walk must wait for the uninterruptible syscall.
        if let Some(pending) = self.pending.take() { let _ = pending.finish(); }
    }
}
fn invalid()->io::Error {io::Error::other("invalid bulk metadata record")}
fn word(b:&[u8],p:usize)->io::Result<u32> {Ok(u32::from_ne_bytes(b.get(p..p+4).ok_or_else(invalid)?.try_into().unwrap()))}
fn wide(b:&[u8],p:usize)->io::Result<u64> {Ok(u64::from_ne_bytes(b.get(p..p+8).ok_or_else(invalid)?.try_into().unwrap()))}
fn parse(b:&[u8])->io::Result<(CString,Option<Metadata>)> {
    let common=word(b,4)?; let file=word(b,16)?;
    if common&libc::ATTR_CMN_NAME==0 {return Err(invalid());}
    let mut p=24;
    let delta=word(b,p)? as i32; let length=word(b,p+4)? as usize;
    let start=(p as i64).checked_add(delta as i64).filter(|n|*n>=0).ok_or_else(invalid)? as usize;
    let end=start.checked_add(length).ok_or_else(invalid)?;
    let bytes=b.get(start..end).filter(|s|s.last()==Some(&0)).ok_or_else(invalid)?;
    let name=CString::new(&bytes[..bytes.len()-1]).map_err(|_|invalid())?;
    if name.as_bytes().is_empty() || name.as_bytes().contains(&b'/') {return Err(invalid());}
    p+=8;
    let mut device=None;let mut kind=None;let mut mtime=None;let mut flags=None;let mut inode=None;
    if common&libc::ATTR_CMN_DEVID!=0 {device=Some(word(b,p)? as u64);p+=4;}
    if common&libc::ATTR_CMN_OBJTYPE!=0 {kind=Some(word(b,p)?);p+=4;}
    if common&libc::ATTR_CMN_MODTIME!=0 {mtime=Some(wide(b,p)? as i64);p+=16;}
    if common&libc::ATTR_CMN_FLAGS!=0 {flags=Some(word(b,p)?);p+=4;}
    if common&libc::ATTR_CMN_FILEID!=0 {inode=Some(wide(b,p)?);p+=8;}
    let mut links=None;let mut allocated=None;let mut logical=None;
    if file&libc::ATTR_FILE_LINKCOUNT!=0 {links=Some(word(b,p)? as u64);p+=4;}
    if file&libc::ATTR_FILE_ALLOCSIZE!=0 {allocated=Some(wide(b,p)?);p+=8;}
    if file&libc::ATTR_FILE_DATALENGTH!=0 {logical=Some(wide(b,p)?);}
    // Directories/firmlinks, symlinks and special nodes retain stat semantics.
    let metadata=if kind==Some(1) {device.zip(inode).zip(links).zip(logical).zip(allocated).zip(flags).zip(mtime)
        .map(|((((((device,inode),links),logical),allocated),flags),mtime)|Metadata{mode:libc::S_IFREG,device,inode,links,logical,allocated,flags,mtime})}else{None};
    Ok((name,metadata))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn later_worker_error_keeps_bulk_used_and_releases_slot() {
        use std::os::fd::AsRawFd;
        let worker = crate::prefetch::Worker::new().unwrap();
        let path = std::env::temp_dir().join(format!("macscan-prefetch-error-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).unwrap();
        let mut bulk = Bulk::new();
        bulk.used = true;
        bulk.pending = worker.submit(file.as_raw_fd(), &mut bulk.buffer);
        assert!(bulk.pending.is_some());
        assert!(bulk.next(file.as_raw_fd()).is_err());
        // Platform fallback is explicitly only !used; later errors remain errors.
        assert!(bulk.used);
        assert!(bulk.pending.is_none());
        assert_eq!(bulk.buffer.len(),64*1024);
        let pending = worker.submit(file.as_raw_fd(), &mut bulk.buffer).unwrap();
        assert!(pending.finish().1.is_err());
        drop(worker);
        drop(file);
        std::fs::remove_file(path).unwrap();
    }
    #[test] fn malformed_records_are_rejected(){
        assert!(parse(&[]).is_err());
        let mut b=vec![0u8;40];b[4..8].copy_from_slice(&libc::ATTR_CMN_NAME.to_ne_bytes());
        b[24..28].copy_from_slice(&i32::MAX.to_ne_bytes());b[28..32].copy_from_slice(&100u32.to_ne_bytes());
        assert!(parse(&b).is_err());
    }
}

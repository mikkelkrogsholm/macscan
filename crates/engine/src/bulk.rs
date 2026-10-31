//! Bounded Darwin metadata batches. Unavailable attributes fall back to fstatat.
use crate::platform::Metadata;
use std::{collections::VecDeque, ffi::CString, io};

const COMMON: u32 = libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_NAME
    | libc::ATTR_CMN_DEVID | libc::ATTR_CMN_OBJTYPE | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_FLAGS | libc::ATTR_CMN_FILEID;
pub(crate) const CLONE_ID:u32=0x100;
pub(crate) const EXT_FLAGS:u32=0x200;
pub(crate) const DATA_ALLOC:u32=0x400;
const EXTENDED:u64=0x20;
const FILE: u32 = libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_ALLOCSIZE | libc::ATTR_FILE_DATALENGTH;
pub(crate) struct Bulk {
    buffer: Vec<u8>,
    rows: VecDeque<(CString, Option<Metadata>, crate::CloneMetadata)>,
    pub used: bool,
    pub capture_clones: bool,
    pending: Option<crate::prefetch::Pending>,
}
impl Bulk {
    pub fn new() -> Self { Self { buffer: vec![0; 64*1024], rows: VecDeque::new(), used:false, capture_clones:false, pending:None } }
    pub fn next(&mut self, fd:i32) -> io::Result<Option<(CString,Option<Metadata>,crate::CloneMetadata)>> {
        if let Some(row)=self.rows.pop_front() {return Ok(Some(row));}
        if !self.used && crate::profile::force_fallback() {
            return Err(io::Error::from_raw_os_error(libc::ENOTSUP));
        }
        let n = if let Some(pending) = self.pending.take() {
            let (buffer, result) = pending.finish();
            self.buffer = buffer;
            result?
        } else {
            crate::profile::timed(crate::profile::Call::Bulk, || read_batch_capture(fd, &mut self.buffer, self.capture_clones))?
        };
        self.used=true;
        let mut offset=0;
        for _ in 0..n {
            let length=word(&self.buffer,offset)? as usize;
            if length<24 || length>self.buffer.len()-offset {return Err(invalid());}
            let record=&self.buffer[offset..offset+length];
            self.rows.push_back(parse_capture(record, self.capture_clones)?);
            offset+=length;
        }
        // Do not reserve the global slot across a directory descent. Batches
        // containing nonregular/missing metadata stay serial; all-file batches
        // can overlap row processing/Parquet callbacks with their next batch.
        if !self.capture_clones && n > 0 && self.rows.iter().all(|(_, metadata, _)| metadata.is_some()) {
            self.pending = crate::profile::prefetch(fd, &mut self.buffer);
        }
        Ok(self.rows.pop_front())
    }
}
// Both coordinator and experimental helper use this exact public Darwin call.
pub(crate) fn read_batch(fd: i32, buffer: &mut [u8]) -> io::Result<usize> {
    read_batch_capture(fd, buffer, false)
}
fn read_batch_capture(fd:i32, buffer:&mut [u8], capture:bool)->io::Result<usize> {
    let mut attrs=libc::attrlist {bitmapcount:5,reserved:0,commonattr:COMMON,volattr:0,dirattr:0,fileattr:FILE|if capture {DATA_ALLOC}else{0},forkattr:if capture {CLONE_ID|EXT_FLAGS}else{0}};
    // SAFETY: caller owns a live directory fd and a bounded writable buffer.
    let n=unsafe {libc::getattrlistbulk(fd,(&mut attrs as *mut libc::attrlist).cast(),buffer.as_mut_ptr().cast(),buffer.len(),if capture {EXTENDED}else{0})};
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
#[cfg(test)]
fn parse(b:&[u8])->io::Result<(CString,Option<Metadata>)> {
    parse_capture(b,false).map(|(n,m,_)|(n,m))
}
fn parse_capture(b:&[u8],capture:bool)->io::Result<(CString,Option<Metadata>,crate::CloneMetadata)> {
    if word(b,0)? as usize!=b.len(){return Err(invalid());}
    let common=word(b,4)?; let file=word(b,16)?; let extended=word(b,20)?;
    if common & !COMMON != 0 || word(b,8)?!=0 || word(b,12)?!=0 || file & !(FILE|if capture {DATA_ALLOC}else{0})!=0 {return Err(invalid());}
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
    if file&libc::ATTR_FILE_DATALENGTH!=0 {logical=Some(wide(b,p)?);p+=8;}
    let mut clone_metadata=crate::CloneMetadata::default();
    if capture {
        // Supplemental failures do not discard coherent primary metadata.
        let supplemental=(||->io::Result<crate::CloneMetadata>{
            let mut take=|mask:bool|->io::Result<Option<u64>> {if mask {let n=wide(b,p)?;p+=8;Ok(Some(n))}else{Ok(None)}};
            let data=take(file&DATA_ALLOC!=0)?;
            let clone_id=take(extended&CLONE_ID!=0)?;
            let flags=take(extended&EXT_FLAGS!=0)?;
            if extended & !(CLONE_ID|EXT_FLAGS)!=0 || start<p || data.is_some_and(|n|n>i64::MAX as u64) || data.zip(allocated).is_some_and(|(d,a)|d>a) {return Err(invalid());}
            let status=if clone_id.is_some()&&flags.is_some()&&data.is_some()&&device.is_some()&&allocated.is_some(){crate::CloneStatus::Observed}else{crate::CloneStatus::Missing};
            Ok(crate::CloneMetadata{status,clone_id,extended_flags:flags,data_allocated:data,device})
        })();
        clone_metadata=supplemental.unwrap_or_else(|_|crate::CloneMetadata::unavailable(crate::CloneStatus::Invalid));
    }
    // Negative off_t values cannot be used for primary allocation either.
    if allocated.is_some_and(|n|n>i64::MAX as u64) || logical.is_some_and(|n|n>i64::MAX as u64) {return Ok((name,None,if capture {crate::CloneMetadata::unavailable(crate::CloneStatus::Invalid)}else{crate::CloneMetadata::default()}));}
    // Directories/firmlinks, symlinks and special nodes retain stat semantics.
    let metadata=if kind==Some(1) {device.zip(inode).zip(links).zip(logical).zip(allocated).zip(flags).zip(mtime)
        .map(|((((((device,inode),links),logical),allocated),flags),mtime)|Metadata{mode:libc::S_IFREG,device,inode,links,logical,allocated,flags,mtime,clone_metadata})}else{None};
    Ok((name,metadata,clone_metadata))
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

#[cfg(test)]
mod clone_tests {
    use super::*;
    fn fixture(file:u32,extended:u32,total:i64,data:i64,id:u64,flags:u64)->Vec<u8> {
        let mut b=vec![0;32];
        b[4..8].copy_from_slice(&COMMON.to_ne_bytes());
        b[16..20].copy_from_slice(&file.to_ne_bytes());
        b[20..24].copy_from_slice(&extended.to_ne_bytes());
        b.extend_from_slice(&7u32.to_ne_bytes()); // coherent device
        b.extend_from_slice(&1u32.to_ne_bytes()); // VREG
        b.extend_from_slice(&3i64.to_ne_bytes());b.extend_from_slice(&0i64.to_ne_bytes());
        b.extend_from_slice(&0u32.to_ne_bytes());b.extend_from_slice(&11u64.to_ne_bytes());
        if file&libc::ATTR_FILE_LINKCOUNT!=0 {b.extend_from_slice(&2u32.to_ne_bytes());}
        if file&libc::ATTR_FILE_ALLOCSIZE!=0 {b.extend_from_slice(&total.to_ne_bytes());}
        if file&libc::ATTR_FILE_DATALENGTH!=0 {b.extend_from_slice(&123u64.to_ne_bytes());}
        if file&DATA_ALLOC!=0 {b.extend_from_slice(&data.to_ne_bytes());}
        if extended&CLONE_ID!=0 {b.extend_from_slice(&id.to_ne_bytes());}
        if extended&EXT_FLAGS!=0 {b.extend_from_slice(&flags.to_ne_bytes());}
        let start=b.len();b.extend_from_slice(b"x\0");
        let length=b.len() as u32;b[..4].copy_from_slice(&length.to_ne_bytes());
        b[24..28].copy_from_slice(&((start-24) as u32).to_ne_bytes());b[28..32].copy_from_slice(&2u32.to_ne_bytes());b
    }
    #[test] fn returned_layout_preserves_zero_and_missing_fields() {
        let b=fixture(FILE|DATA_ALLOC,CLONE_ID|EXT_FLAGS,8192,4096,0,65);
        let (_,m,c)=parse_capture(&b,true).unwrap();let m=m.unwrap();
        assert_eq!((m.logical,m.allocated,m.device),(123,8192,7));
        assert_eq!((c.status,c.clone_id,c.extended_flags,c.data_allocated,c.device),(crate::CloneStatus::Observed,Some(0),Some(65),Some(4096),Some(7)));
        for (f,x) in [(FILE,CLONE_ID|EXT_FLAGS),(FILE|DATA_ALLOC,CLONE_ID),(FILE|DATA_ALLOC,EXT_FLAGS)] {
            let (_,m,c)=parse_capture(&fixture(f,x,8192,4096,11,65),true).unwrap();
            assert_eq!(m.unwrap().allocated,8192);assert_eq!(c.status,crate::CloneStatus::Missing);
            assert_eq!(c.clone_id.is_some(),x&CLONE_ID!=0);assert_eq!(c.extended_flags.is_some(),x&EXT_FLAGS!=0);
        }
    }
    #[test] fn missing_total_cannot_certify_data_against_fallback_stat() {
        let (_,m,c)=parse_capture(&fixture((FILE&!libc::ATTR_FILE_ALLOCSIZE)|DATA_ALLOC,CLONE_ID|EXT_FLAGS,8192,4096,11,65),true).unwrap();
        assert!(m.is_none());assert_eq!(c.status,crate::CloneStatus::Missing);
        assert_eq!(c.data_allocated,Some(4096));assert_eq!(c.device,Some(7));
    }
    #[test] fn invalid_supplemental_values_preserve_primary_metadata() {
        for data in [-1,8193] {
            let (_,m,c)=parse_capture(&fixture(FILE|DATA_ALLOC,CLONE_ID|EXT_FLAGS,8192,data,22,65),true).unwrap();
            assert_eq!(m.unwrap().allocated,8192);assert_eq!(c.status,crate::CloneStatus::Invalid);assert!(c.clone_id.is_none());
        }
        let mut b=fixture(FILE,0,8192,0,0,0);
        b[16..20].copy_from_slice(&(FILE|DATA_ALLOC).to_ne_bytes());
        let (_,m,c)=parse_capture(&b,true).unwrap();assert_eq!(m.unwrap().logical,123);assert_eq!(c.status,crate::CloneStatus::Invalid);
        let (_,m,c)=parse_capture(&fixture(FILE|DATA_ALLOC,CLONE_ID|EXT_FLAGS,-1,1,22,65),true).unwrap();
        assert!(m.is_none());assert_eq!(c.status,crate::CloneStatus::Invalid);
    }
    #[test] fn baseline_is_unchanged_and_cross_device_raw_values_stay_distinct() {
        let (_,m,c)=parse_capture(&fixture(FILE,0,8192,0,0,0),false).unwrap();
        assert_eq!(m.unwrap().allocated,8192);assert_eq!(c,crate::CloneMetadata::default());
        let b=fixture(FILE|DATA_ALLOC,CLONE_ID|EXT_FLAGS,8192,4096,33,65);
        let a=parse_capture(&b,true).unwrap().2;
        let mut b=b;b[32..36].copy_from_slice(&8u32.to_ne_bytes());let z=parse_capture(&b,true).unwrap().2;
        assert_eq!(a.clone_id,z.clone_id);assert_ne!(a.device,z.device);
        assert!(parse_capture(&b[..40],true).is_err());
    }
}

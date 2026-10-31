//! Benchmark-only single global slot. No DIR pointer crosses a thread boundary.
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

type Batch = (Vec<u8>, io::Result<usize>, Duration);
struct Job {
    fd: OwnedFd,
    buffer: Vec<u8>,
    reply: mpsc::SyncSender<Batch>,
}
pub(crate) struct Worker {
    sender: Option<mpsc::SyncSender<Job>>,
    handle: Option<thread::JoinHandle<()>>,
    occupied: Arc<AtomicBool>,
}
pub(crate) struct Pending {
    reply: mpsc::Receiver<Batch>,
    occupied: Arc<AtomicBool>,
}
impl Worker {
    pub fn new() -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Job>(1);
        let handle = thread::Builder::new()
            .name("macscan-bulk-experiment".into())
            .spawn(move || {
                while let Ok(mut job) = receiver.recv() {
                    let start = Instant::now();
                    let result = crate::bulk::read_batch(job.fd.as_raw_fd(), &mut job.buffer);
                    let elapsed = start.elapsed();
                    // Release descriptor before making the result observable.
                    drop(job.fd);
                    let _ = job.reply.send((job.buffer, result, elapsed));
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            handle: Some(handle),
            occupied: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn submit(&self, fd: i32, buffer: &mut Vec<u8>) -> Option<Pending> {
        // Retained parent result keeps the slot occupied; descendants run serially.
        if self.occupied.swap(true, Ordering::AcqRel) {
            return None;
        }
        // dup shares the directory offset, but only this helper enumerates it until
        // Pending::finish. The coordinator may do descriptor-relative stat/open.
        let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if duplicate < 0 {
            self.occupied.store(false, Ordering::Release);
            return None;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(duplicate) };
        let (reply, receiver) = mpsc::sync_channel(1);
        let job = Job {
            fd,
            buffer: std::mem::take(buffer),
            reply,
        };
        if let Err(error) = self.sender.as_ref().unwrap().send(job) {
            *buffer = error.0.buffer;
            self.occupied.store(false, Ordering::Release);
            return None;
        }
        Some(Pending {
            reply: receiver,
            occupied: self.occupied.clone(),
        })
    }
}
impl Pending {
    pub fn finish(self) -> (Vec<u8>, io::Result<usize>) {
        let batch = self.reply.recv();
        self.occupied.store(false, Ordering::Release);
        match batch {
            Ok((buffer, result, elapsed)) => {
                crate::profile::worker_finished(elapsed, result.as_ref().copied().unwrap_or(0));
                (buffer, result)
            }
            Err(_) => (
                vec![0; 64 * 1024],
                Err(io::Error::other("bulk helper terminated")),
            ),
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        drop(self.sender.take());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_slot_including_retained_result_and_owned_fd_errors() {
        let root = std::env::temp_dir().join(format!(
            "macscan-prefetch-slot-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("owned-file"), b"fixture").unwrap();
        let directory = std::fs::File::open(&root).unwrap();
        let worker = Worker::new().unwrap();
        let mut a = vec![0; 64 * 1024];
        let pending = worker.submit(directory.as_raw_fd(), &mut a).unwrap();
        assert!(a.is_empty());
        let mut b = vec![0; 64 * 1024];
        assert!(worker.submit(directory.as_raw_fd(), &mut b).is_none());
        assert_eq!(b.len(), 64 * 1024);
        let (buffer, result) = pending.finish();
        assert_eq!(buffer.len(), 64 * 1024);
        result.unwrap();
        // An owned ordinary file is duplicable but cannot be bulk-enumerated.
        let regular = std::fs::File::open(root.join("owned-file")).unwrap();
        let pending = worker.submit(regular.as_raw_fd(), &mut b).unwrap();
        let (_, result) = pending.finish();
        assert!(result.is_err());
        assert!(!worker.occupied.load(Ordering::Acquire));
        drop(worker); // joins, also after error
        drop(directory);
        drop(regular);
        std::fs::remove_dir_all(root).unwrap();
    }
}

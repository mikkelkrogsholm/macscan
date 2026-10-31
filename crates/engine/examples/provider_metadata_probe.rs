//! Opt-in owned-local-fixture experiment, never a scan entry point.
//! No path arguments or provider access. See docs/provider-metadata-findings.md.
use serde_json::{json, Value};
use std::{
    ffi::CString,
    fs, io,
    mem::MaybeUninit,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TYPE: i32 = 3; // IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, sys/resource.h
const THREAD: i32 = 1; // IOPOL_SCOPE_THREAD; never PROCESS
const OFF: i32 = 1; // IOPOL_MATERIALIZE_DATALESS_FILES_OFF
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}
trait Policy {
    fn get(&self) -> io::Result<i32>;
    fn set(&self, value: i32) -> io::Result<()>;
}
extern "C" {
    fn getiopolicy_np(iotype: libc::c_int, scope: libc::c_int) -> libc::c_int;
    fn setiopolicy_np(iotype: libc::c_int, scope: libc::c_int, policy: libc::c_int) -> libc::c_int;
}
struct Native;
impl Policy for Native {
    fn get(&self) -> io::Result<i32> {
        // SAFETY: public scalar API, current thread scope only.
        let n = unsafe { getiopolicy_np(TYPE, THREAD) };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n)
        }
    }
    fn set(&self, value: i32) -> io::Result<()> {
        // SAFETY: public scalar API, current thread scope only.
        if unsafe { setiopolicy_np(TYPE, THREAD, value) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}
fn failure(stage: &str, e: &io::Error) -> Value {
    json!({"stage":stage,"errno":e.raw_os_error(),"kind":match e.raw_os_error() {
        Some(libc::EDEADLK)=>"materialization_blocked",Some(libc::EACCES)|Some(libc::EPERM)=>"access_denied",
        Some(libc::EINVAL)|Some(libc::ENOTSUP)|Some(libc::ENOSYS)=>"unsupported",
        _ if e.kind()==io::ErrorKind::Interrupted=>"cancelled",_=>"unavailable"
    },"detail":e.to_string()})
}
// A small probe-local control-flow helper, not a product policy abstraction.
// catch_unwind ensures explicit restoration/reporting after an operation panic.
fn guarded(policy: &impl Policy, operation: impl FnOnce() -> io::Result<Value>) -> Value {
    let original = match policy.get() {
        Ok(n) => n,
        Err(e) => {
            return json!({"setup":failure("get_original",&e),"target_calls":false,"restoration":"not_needed"})
        }
    };
    let setup = policy.set(OFF).and_then(|()| {
        let n = policy.get()?;
        if n == OFF {
            Ok(())
        } else {
            Err(io::Error::other(format!("OFF not confirmed: {n}")))
        }
    });
    let (result, called) = match setup {
        Err(e) => (json!({"setup":failure("set_or_confirm_off",&e)}), false),
        Ok(()) => match catch_unwind(AssertUnwindSafe(operation)) {
            Ok(Ok(v)) => (json!({"operation":v}), true),
            Ok(Err(e)) => (json!({"operation_error":failure("owned_fixture",&e)}), true),
            Err(_) => (json!({"operation_error":{"kind":"unwind"}}), true),
        },
    };
    // Even failed set/confirmation attempts are followed by exact restoration.
    let restored = policy.set(original).and_then(|()| {
        let n = policy.get()?;
        if n == original {
            Ok(n)
        } else {
            Err(io::Error::other(format!(
                "restore mismatch: expected {original}, got {n}"
            )))
        }
    });
    json!({"scope":"dedicated_thread","original":original,"confirmed_off":called,"target_calls":called,"result":result,
        "restoration":match restored {Ok(n)=>json!({"status":"verified","observed":n}),Err(e)=>failure("restore_or_confirm",&e)}})
}
struct Budget {
    start: Instant,
    calls: usize,
}
impl Budget {
    fn call<T>(&mut self, name: &str, f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        if STOP.load(Ordering::Relaxed)
            || self.start.elapsed() >= Duration::from_secs(5)
            || self.calls >= 32
        {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "stopped before next call",
            ));
        }
        self.calls += 1;
        let result = f();
        println!(
            "{}",
            json!({"call":self.calls,"operation":name,"ok":result.is_ok(),"error":result.as_ref().err().map(|e|failure(name,e))})
        );
        result
    }
}
// Only these three owned paths are ever removed; no recursive cleanup or user path.
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn cleanup(&self) -> io::Result<()> {
        for (path, dir) in [
            (self.0.join("synthetic"), false),
            (self.0.join("child"), true),
            (self.0.clone(), true),
        ] {
            let r = if dir {
                fs::remove_dir(path)
            } else {
                fs::remove_file(path)
            };
            if let Err(e) = r {
                if e.kind() != io::ErrorKind::NotFound {
                    return Err(e);
                }
            }
        }
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(e) = self.cleanup() {
            eprintln!("{}", failure("cleanup", &e));
        }
    }
}
fn owned_local_probe() -> io::Result<Value> {
    let mut b = Budget {
        start: Instant::now(),
        calls: 0,
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::path::PathBuf::from(format!(
        "/private/tmp/macscan-provider-probe-{}-{nonce}",
        std::process::id()
    ));
    b.call("create_owned_directory", || fs::create_dir(&path))?;
    let fixture = Fixture(path);
    let dir = b.call("open_owned_directory", || fs::File::open(&fixture.0))?;
    let mut stat = MaybeUninit::<libc::statfs>::uninit();
    b.call("fstatfs_owned_directory", || {
        // SAFETY: live owned fd and writable output.
        if unsafe { libc::fstatfs(dir.as_raw_fd(), stat.as_mut_ptr()) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    })?;
    // SAFETY: successful fstatfs initialized output.
    let stat = unsafe { stat.assume_init() };
    if stat.f_flags & libc::MNT_LOCAL as u32 == 0 {
        return Err(io::Error::other("owned temporary location is not local"));
    }
    b.call("write_own_synthetic_bytes", || {
        fs::write(
            fixture.0.join("synthetic"),
            b"provider probe synthetic fixture\n",
        )
    })?;
    b.call("create_owned_child", || {
        fs::create_dir(fixture.0.join("child"))
    })?;
    let m = b.call("symlink_metadata_owned", || {
        fs::symlink_metadata(fixture.0.join("synthetic"))
    })?;
    let canonical = b.call("canonicalize_owned", || fixture.0.canonicalize())?;
    let identity = b.call("fstat_owned_directory", || dir.metadata())?;
    let mut attrs = libc::attrlist {
        bitmapcount: 5,
        reserved: 0,
        commonattr: libc::ATTR_CMN_RETURNED_ATTRS | libc::ATTR_CMN_NAME | libc::ATTR_CMN_FLAGS,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut bytes = [0u8; 4096];
    let bulk = b.call("getattrlistbulk_owned", || {
        // SAFETY: public attrlist and bounded output; owned directory fd.
        let n = unsafe {
            libc::getattrlistbulk(
                dir.as_raw_fd(),
                (&mut attrs as *mut libc::attrlist).cast(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                0,
            )
        };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n)
        }
    })?;
    let mut entries = b.call("read_dir_owned", || fs::read_dir(&fixture.0))?;
    let mut names = 0;
    while let Some(e) = b.call("next_readdir_owned", || entries.next().transpose())? {
        let _ = e.file_name();
        names += 1;
    }
    for (name, flags) in [
        ("synthetic", libc::AT_SYMLINK_NOFOLLOW),
        ("child", libc::AT_SYMLINK_NOFOLLOW),
    ] {
        let name = CString::new(name).unwrap();
        let mut s = MaybeUninit::<libc::stat>::uninit();
        b.call("fstatat_owned", || {
            // SAFETY: direct synthetic name, live owned fd and output.
            if unsafe { libc::fstatat(dir.as_raw_fd(), name.as_ptr(), s.as_mut_ptr(), flags) } == 0
            {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        })?;
    }
    let child = CString::new("child").unwrap();
    let child_fd = b.call("openat_owned_child", || {
        // SAFETY: direct owned directory entry; no file content access.
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                child.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(fd)
        }
    })?;
    // SAFETY: child_fd is uniquely owned, close once.
    unsafe {
        libc::close(child_fd);
    }
    let final_identity = b.call("final_fstat_owned", || dir.metadata())?;
    let same = (identity.dev(), identity.ino()) == (final_identity.dev(), final_identity.ino());
    drop(entries);
    drop(dir);
    fixture.cleanup()?;
    println!(
        "{}",
        json!({"cleanup":"removed_owned_fixture","fixture":fixture.0})
    );
    Ok(
        json!({"fixture":"owned_local_only","canonical_matches":canonical==fixture.0,"logical":m.len(),"allocated":m.blocks()*512,"bulk_entries":bulk,"readdir_entries":names,"identity_stable":same,"bounded_calls":b.calls}),
    )
}
#[cfg(test)]
#[derive(Clone, Copy)]
struct Observation {
    provider: Option<bool>,
    dataless: bool,
    directory: bool,
    logical: Option<u64>,
    allocated: Option<u64>,
}
#[cfg(test)]
fn classify(o: Observation) -> Value {
    let unknown = o.provider.is_none();
    json!({"state":if unknown{"unknown_provider_root"}else if o.dataless{"dataless_placeholder"}else if o.provider==Some(true){"known_local_provider_file"}else{"ordinary_local_nonprovider"},
        "logical":if unknown{None}else{o.logical},"allocated":if unknown{None}else{o.allocated},
        "traverse":o.directory && !unknown && !o.dataless,"coverage":if unknown||o.dataless{"partial"}else{"observed"}})
}
#[cfg(test)]
fn comparable(a: &str, b: &str, covered: bool) -> bool {
    a == b && covered
}
fn main() -> io::Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args != ["--owned-local-fixture"] {
        println!("Opt-in only: --owned-local-fixture (no path arguments); no fixture created.");
        return Ok(());
    }
    // SAFETY: handler only stores an atomic flag; Stop is checked between calls.
    unsafe {
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
    }
    let caller = Native
        .get()
        .map_err(|e| io::Error::other(format!("caller policy unavailable: {e}")))?;
    let result = std::thread::Builder::new()
        .name("provider-metadata-probe".into())
        .spawn(|| guarded(&Native, owned_local_probe))?
        .join()
        .map_err(|_| io::Error::other("probe thread panicked outside operation"))?;
    let caller_after = Native.get()?;
    println!(
        "{}",
        json!({"report":result,"caller_policy_before":caller,"caller_policy_after":caller_after,"caller_unchanged":caller==caller_after})
    );
    if result["restoration"]["status"] != "verified"
        || result["result"]["operation"].is_null()
        || caller != caller_after
    {
        return Err(io::Error::other("probe did not pass; see report"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::VecDeque};
    struct Fake {
        gets: RefCell<VecDeque<Result<i32, i32>>>,
        sets: RefCell<VecDeque<Result<(), i32>>>,
        written: RefCell<Vec<i32>>,
    }
    impl Fake {
        fn new(gets: Vec<Result<i32, i32>>, sets: Vec<Result<(), i32>>) -> Self {
            Self {
                gets: RefCell::new(gets.into()),
                sets: RefCell::new(sets.into()),
                written: RefCell::new(vec![]),
            }
        }
    }
    impl Policy for Fake {
        fn get(&self) -> io::Result<i32> {
            self.gets
                .borrow_mut()
                .pop_front()
                .expect("unexpected get")
                .map_err(io::Error::from_raw_os_error)
        }
        fn set(&self, n: i32) -> io::Result<()> {
            self.written.borrow_mut().push(n);
            self.sets
                .borrow_mut()
                .pop_front()
                .expect("unexpected set")
                .map_err(io::Error::from_raw_os_error)
        }
    }
    #[test]
    fn unsupported_get_fails_before_operations() {
        let f = Fake::new(vec![Err(libc::ENOTSUP)], vec![]);
        let r = guarded(&f, || panic!("must not call"));
        assert_eq!(r["target_calls"], false);
        assert!(f.written.borrow().is_empty());
    }
    #[test]
    fn unsupported_set_restores_and_never_calls() {
        let f = Fake::new(vec![Ok(2), Ok(2)], vec![Err(libc::EINVAL), Ok(())]);
        let r = guarded(&f, || panic!("must not call"));
        assert_eq!(r["target_calls"], false);
        assert_eq!(*f.written.borrow(), [OFF, 2]);
        assert_eq!(r["restoration"]["status"], "verified");
    }
    #[test]
    fn unconfirmed_off_fails_closed() {
        let f = Fake::new(vec![Ok(2), Ok(0), Ok(2)], vec![Ok(()), Ok(())]);
        let r = guarded(&f, || panic!("must not call"));
        assert_eq!(r["target_calls"], false);
        assert_eq!(r["restoration"]["status"], "verified");
    }
    #[test]
    fn blocked_and_denied_are_unknown_errors_not_zero() {
        for (errno, kind) in [
            (libc::EDEADLK, "materialization_blocked"),
            (libc::EACCES, "access_denied"),
        ] {
            let f = Fake::new(vec![Ok(2), Ok(OFF), Ok(2)], vec![Ok(()), Ok(())]);
            let r = guarded(&f, || Err(io::Error::from_raw_os_error(errno)));
            assert_eq!(r["result"]["operation_error"]["kind"], kind);
            assert_eq!(r["restoration"]["observed"], 2);
        }
    }
    #[test]
    fn operation_unwind_restores_exact_original() {
        let f = Fake::new(vec![Ok(0), Ok(OFF), Ok(0)], vec![Ok(()), Ok(())]);
        let r = guarded(&f, || panic!("injected"));
        assert_eq!(r["result"]["operation_error"]["kind"], "unwind");
        assert_eq!(r["restoration"]["observed"], 0);
    }
    #[test]
    fn restore_failure_or_mismatch_is_visible() {
        for f in [
            Fake::new(vec![Ok(2), Ok(OFF)], vec![Ok(()), Err(libc::EACCES)]),
            Fake::new(vec![Ok(2), Ok(OFF), Ok(OFF)], vec![Ok(()), Ok(())]),
            Fake::new(
                vec![Ok(2), Ok(OFF), Err(libc::ENOTSUP)],
                vec![Ok(()), Ok(())],
            ),
        ] {
            let r = guarded(&f, || Ok(json!({})));
            assert_eq!(r["restoration"]["stage"], "restore_or_confirm");
        }
    }
    #[test]
    fn cancellation_prevents_next_call_and_restores() {
        STOP.store(true, Ordering::Relaxed);
        let f = Fake::new(vec![Ok(2), Ok(OFF), Ok(2)], vec![Ok(()), Ok(())]);
        let r = guarded(&f, || {
            Budget {
                start: Instant::now(),
                calls: 0,
            }
            .call("never", || panic!("must not call"))
        });
        STOP.store(false, Ordering::Relaxed);
        assert_eq!(r["result"]["operation_error"]["kind"], "cancelled");
        assert_eq!(r["restoration"]["observed"], 2);
    }
    #[test]
    fn classification_and_policy_evolution() {
        let placeholder = Observation {
            provider: Some(true),
            dataless: true,
            directory: true,
            logical: Some(42),
            allocated: None,
        };
        let r = classify(placeholder);
        assert_eq!(r["logical"], 42);
        assert!(r["allocated"].is_null());
        assert_eq!(r["traverse"], false);
        let local = classify(Observation {
            dataless: false,
            directory: false,
            allocated: Some(4096),
            ..placeholder
        });
        assert_eq!(local["state"], "known_local_provider_file");
        assert_eq!(local["allocated"], 4096);
        let unknown = classify(Observation {
            provider: None,
            ..placeholder
        });
        assert!(unknown["logical"].is_null());
        assert!(unknown["allocated"].is_null());
        assert_eq!(unknown["traverse"], false);
        let ordinary = classify(Observation {
            provider: Some(false),
            dataless: false,
            directory: true,
            logical: Some(0),
            allocated: Some(0),
        });
        assert_eq!(ordinary["state"], "ordinary_local_nonprovider");
        assert_eq!(ordinary["traverse"], true);
        assert!(!comparable(
            "cloud-broad-excluded-v1",
            "cloud-partly-observed-off-v2",
            true
        ));
        assert!(!comparable("v2", "v2", false));
        assert!(comparable("v2", "v2", true));
    }
}

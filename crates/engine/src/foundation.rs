//! Narrow public Foundation bridge: local volume resource values only.
//! No document reads, network URLs, plugins, callbacks, or user code execution.
use std::ffi::{c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

type Object = *mut c_void;
type Selector = *mut c_void;
#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Object;
    fn sel_registerName(name: *const c_char) -> Selector;
    fn objc_msgSend();
}
#[link(name = "Foundation", kind = "framework")]
extern "C" {
    static NSURLVolumeUUIDStringKey: Object;
    static NSURLVolumeAvailableCapacityForImportantUsageKey: Object;
}

#[derive(Default)]
pub(crate) struct Resources {
    pub uuid: Option<String>,
    pub important: Option<u64>,
}

// SAFETY obligations at all call sites: receivers are Foundation objects of the
// named class, selectors have exactly the typed ABI below, and the autorelease
// pool remains alive until all returned values have been copied to Rust.
unsafe fn message0(receiver: Object, selector: Selector) -> Object {
    let send: unsafe extern "C" fn(Object, Selector) -> Object =
        unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    unsafe { send(receiver, selector) }
}
struct Pool(Object);
impl Drop for Pool {
    fn drop(&mut self) {
        let send: unsafe extern "C" fn(Object, Selector) =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe {
            send(self.0, sel_registerName(c"drain".as_ptr()));
        }
    }
}

pub(crate) fn volume_resources(path: &Path) -> Resources {
    let bytes = path.as_os_str().as_bytes();
    if !path.is_absolute() || bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
        return Resources::default();
    }
    // All external messages below have fixed, documented Foundation selectors.
    // fileURLWithPath constructs a local URL; no remote URL is ever accepted.
    unsafe {
        let pool = message0(
            objc_getClass(c"NSAutoreleasePool".as_ptr()),
            sel_registerName(c"new".as_ptr()),
        );
        if pool.is_null() {
            return Resources::default();
        }
        let _pool = Pool(pool);
        let string_class = objc_getClass(c"NSString".as_ptr());
        let allocated = message0(string_class, sel_registerName(c"alloc".as_ptr()));
        let init: unsafe extern "C" fn(Object, Selector, *const u8, usize, usize) -> Object =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let path_string = init(
            allocated,
            sel_registerName(c"initWithBytes:length:encoding:".as_ptr()),
            bytes.as_ptr(),
            bytes.len(),
            4,
        );
        if path_string.is_null() {
            return Resources::default();
        }
        let _ = message0(path_string, sel_registerName(c"autorelease".as_ptr()));
        let send_url: unsafe extern "C" fn(Object, Selector, Object) -> Object =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let url = send_url(
            objc_getClass(c"NSURL".as_ptr()),
            sel_registerName(c"fileURLWithPath:".as_ptr()),
            path_string,
        );
        if url.is_null() {
            return Resources::default();
        }
        let get: unsafe extern "C" fn(Object, Selector, *mut Object, Object, *mut Object) -> i8 =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let selector = sel_registerName(c"getResourceValue:forKey:error:".as_ptr());
        let mut uuid_object = std::ptr::null_mut();
        let mut important_object = std::ptr::null_mut();
        let mut error = std::ptr::null_mut();
        let uuid = if get(
            url,
            selector,
            &mut uuid_object,
            NSURLVolumeUUIDStringKey,
            &mut error,
        ) != 0
            && !uuid_object.is_null()
        {
            string_copy(uuid_object)
        } else {
            None
        };
        let important = if get(
            url,
            selector,
            &mut important_object,
            NSURLVolumeAvailableCapacityForImportantUsageKey,
            &mut error,
        ) != 0
            && !important_object.is_null()
        {
            let number: unsafe extern "C" fn(Object, Selector) -> i64 =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            u64::try_from(number(
                important_object,
                sel_registerName(c"longLongValue".as_ptr()),
            ))
            .ok()
        } else {
            None
        };
        Resources { uuid, important }
    }
}

unsafe fn string_copy(object: Object) -> Option<String> {
    let length: unsafe extern "C" fn(Object, Selector, usize) -> usize =
        unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    let len = unsafe {
        length(
            object,
            sel_registerName(c"lengthOfBytesUsingEncoding:".as_ptr()),
            4,
        )
    };
    if len == 0 || len > 128 {
        return None;
    }
    let utf8: unsafe extern "C" fn(Object, Selector) -> *const u8 =
        unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
    let pointer = unsafe { utf8(object, sel_registerName(c"UTF8String".as_ptr())) };
    if pointer.is_null() {
        return None;
    }
    // Foundation guarantees UTF8String remains valid for this live NSString.
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(pointer, len) })
        .ok()
        .map(str::to_owned)
}

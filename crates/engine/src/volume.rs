//! Cached mount inventory and local, individually scoped capacity measurements.
use crate::{foundation, platform, scan::hex, ENGINE_VERSION, SCHEMA_VERSION};
use serde_json::{json, Value};
use std::ffi::{CString, OsStr};
use std::io;
use std::mem::{size_of, MaybeUninit};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const MAX_MOUNTS: usize = 4096;

fn c_bytes<const N: usize>(array: &[libc::c_char; N]) -> io::Result<Vec<u8>> {
    let end = array
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| io::Error::other("unterminated OS string"))?;
    Ok(array[..end].iter().map(|b| *b as u8).collect())
}

pub(crate) fn os_identity() -> io::Result<String> {
    let mut info = MaybeUninit::<libc::utsname>::uninit();
    // SAFETY: uname initializes the entire output on success.
    if unsafe { libc::uname(info.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    Ok(format!(
        "{}:{}:{}",
        String::from_utf8_lossy(&c_bytes(&info.sysname)?),
        String::from_utf8_lossy(&c_bytes(&info.release)?),
        String::from_utf8_lossy(&c_bytes(&info.machine)?)
    ))
}

fn snapshot(stat: &libc::statfs, live: bool) -> io::Result<Value> {
    let captured_at = platform::timestamp();
    let mount = c_bytes(&stat.f_mntonname)?;
    let source = c_bytes(&stat.f_mntfromname)?;
    let fs = c_bytes(&stat.f_fstypename)?;
    let local = stat.f_flags & libc::MNT_LOCAL as u32 != 0;
    let supported =
        local && [b"apfs".as_slice(), b"hfs", b"exfat", b"msdos"].contains(&fs.as_slice());
    // Only true local volume roots reach Foundation, never arbitrary files,
    // provider subtrees, automount triggers, or remote mounts.
    let resources = if supported {
        foundation::volume_resources(Path::new(OsStr::from_bytes(&mount)))
    } else {
        foundation::Resources::default()
    };
    let physical_uuid = if supported {
        native_uuid(Path::new(OsStr::from_bytes(&mount)))
            .ok()
            .flatten()
    } else {
        None
    };
    let block_bytes = |blocks: u64| {
        blocks
            .checked_mul(stat.f_bsize as u64)
            .map(|n| n.to_string())
            .ok_or_else(|| io::Error::other("capacity overflow"))
    };
    // Darwin's public fsid_t is two int32_t values; libc intentionally keeps
    // its fields private. transmute also compile-checks the eight-byte layout.
    let fsid_values: [i32; 2] = unsafe { std::mem::transmute(stat.f_fsid) };
    let fsid = format!("{}:{}", fsid_values[0] as u32, fsid_values[1] as u32);
    Ok(
        json!({"fsid":fsid,"volume_uuid":physical_uuid,"volume_uuid_source":"getattrlist_ATTR_VOL_UUID",
        "foundation_volume_uuid":resources.uuid,"container_id":null,"startup_volume_group_id":null,
        "mount_display":String::from_utf8_lossy(&mount),"mount_path_bytes_hex":hex(&mount),
        "device_display":String::from_utf8_lossy(&source),"filesystem":String::from_utf8_lossy(&fs),
        "local":local,"supported_local_scan":supported,"captured_at":captured_at,"resource_query_ended_at":platform::timestamp(),
        "capacity_source":if live { "fstatfs" } else { "getfsstat_MNT_NOWAIT_cached" },
        "capacity_quality":"measured", "capacity_scope":"this_volume_view_not_summable_across_shared_pools",
        "total_bytes":block_bytes(stat.f_blocks)?,"free_bytes":block_bytes(stat.f_bavail)?,
        "free_bytes_definition":"available_to_current_unprivileged_user_blocks",
        "raw_free_bytes":block_bytes(stat.f_bfree)?,
        "available_for_important_usage_bytes":resources.important.map(|n| n.to_string()),
        "important_usage_source":"NSURLVolumeAvailableCapacityForImportantUsageKey",
        "important_usage_quality":if resources.important.is_some() { "estimated" } else { "unavailable" },
        "important_usage_note":if resources.important.is_some_and(|n| n < stat.f_bavail.saturating_mul(stat.f_bsize as u64)) {
            Some("OS estimate is below current free bytes; not a disk-full diagnosis and not interchangeable with free space")
        } else {None},
        "shared_space_possible":fs == b"apfs", "free_space_sum_allowed":false}),
    )
}

fn native_uuid(path: &Path) -> io::Result<Option<String>> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| crate::query::invalid("NUL in mount path"))?;
    let mut attrs = libc::attrlist {
        bitmapcount: 5,
        reserved: 0,
        commonattr: libc::ATTR_CMN_RETURNED_ATTRS,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_UUID,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut bytes = [0_u8; 64];
    // SAFETY: bounded output and public attrlist layout; direct local volume
    // metadata only. FSOPT_NOFOLLOW_ANY=0x800 prevents symlink path resolution.
    if unsafe {
        libc::getattrlist(
            path.as_ptr(),
            (&mut attrs as *mut libc::attrlist).cast(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
            0x800 | libc::FSOPT_PACK_INVAL_ATTRS,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    parse_uuid(&bytes)
}

fn parse_uuid(bytes: &[u8]) -> io::Result<Option<String>> {
    let word = |offset: usize| -> io::Result<u32> {
        let field = bytes
            .get(offset..offset + 4)
            .ok_or_else(|| io::Error::other("short volume attributes"))?;
        Ok(u32::from_ne_bytes(field.try_into().unwrap()))
    };
    let length = word(0)? as usize;
    if length < 24 || length > bytes.len() {
        return Err(io::Error::other("invalid volume attribute length"));
    }
    if word(8)? & libc::ATTR_VOL_UUID == 0 {
        return Ok(None);
    }
    if length < 40 {
        return Err(io::Error::other("short volume UUID"));
    }
    let uuid = hex(&bytes[24..40]);
    if bytes[24..40].iter().all(|b| *b == 0) {
        return Ok(None);
    }
    Ok(Some(
        format!(
            "{}-{}-{}-{}-{}",
            &uuid[..8],
            &uuid[8..12],
            &uuid[12..16],
            &uuid[16..20],
            &uuid[20..]
        )
        .to_uppercase(),
    ))
}

pub(crate) fn capture_fd(fd: libc::c_int) -> io::Result<Value> {
    let mut stat = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: fd belongs to a live open directory; fstatfs writes on success.
    if unsafe { libc::fstatfs(fd, stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    snapshot(unsafe { &stat.assume_init() }, true)
}

pub fn volumes(limit: usize, cursor: Option<&str>) -> io::Result<Value> {
    if limit == 0 || limit > 500 {
        return Err(crate::query::invalid("volume limit must be 1..500"));
    }
    let mut mounts = None;
    for _ in 0..3 {
        // MNT_NOWAIT uses the cached mount table, not a network/remote refresh.
        let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        let capacity = (count as usize).saturating_add(8);
        if capacity > MAX_MOUNTS {
            return Err(io::Error::other("mount table exceeds bounded inventory"));
        }
        let mut records = Vec::<MaybeUninit<libc::statfs>>::with_capacity(capacity);
        let bytes = i32::try_from(capacity * size_of::<libc::statfs>())
            .map_err(|_| io::Error::other("mount table size overflow"))?;
        // SAFETY: allocated space for capacity statfs entries; OS writes count
        // entries and return count is checked before creating initialized slices.
        let observed =
            unsafe { libc::getfsstat(records.as_mut_ptr().cast(), bytes, libc::MNT_NOWAIT) };
        if observed < 0 {
            return Err(io::Error::last_os_error());
        }
        if observed as usize >= capacity {
            continue;
        }
        unsafe {
            records.set_len(observed as usize);
        }
        mounts = Some(records);
        break;
    }
    let mounts = mounts
        .ok_or_else(|| io::Error::other("mount inventory kept changing; retry explicitly"))?;
    let mut entries = Vec::new();
    for stat in &mounts {
        let stat = unsafe { stat.assume_init_ref() };
        if stat.f_flags & libc::MNT_LOCAL as u32 != 0 {
            entries.push(snapshot(stat, false)?);
        }
    }
    entries.sort_by(|a, b| {
        a["mount_path_bytes_hex"]
            .as_str()
            .cmp(&b["mount_path_bytes_hex"].as_str())
    });
    // Cursor uses last mount path rather than an index into a changing inventory.
    let key = cursor
        .map(|c| {
            c.strip_prefix("volumes-v1:")
                .ok_or_else(|| crate::query::invalid("invalid volumes cursor"))
        })
        .transpose()?;
    let start = key
        .map(|k| entries.partition_point(|e| e["mount_path_bytes_hex"].as_str().unwrap_or("") <= k))
        .unwrap_or(0);
    let mut response = json!({"schema_version":SCHEMA_VERSION,"engine_version":ENGINE_VERSION,"scan_id":null,
        "status":"complete","command":"volumes","metric":"capacity","scope":{"kind":"cached_local_mount_inventory"},
        "coverage":{"complete":true,"network_mounts_not_refreshed_or_traversed":true},
        "records":[],"truncated":false,"next_cursor":null,
        "warnings":[{"code":"SHARED_FREE_SPACE_MUST_NOT_BE_SUMMED"},{"code":"CONTAINER_AND_VOLUME_GROUP_ID_UNAVAILABLE"}]});
    let mut next = start;
    for entry in entries.iter().skip(start).take(limit) {
        response["records"]
            .as_array_mut()
            .unwrap()
            .push(entry.clone());
        if serde_json::to_vec(&response)?.len() + 4096 > crate::OUTPUT_BUDGET {
            response["records"].as_array_mut().unwrap().pop();
            break;
        }
        next += 1;
    }
    if next == start && start < entries.len() {
        return Err(crate::query::invalid(
            "one volume record exceeds output budget",
        ));
    }
    response["truncated"] = json!(next < entries.len());
    response["next_cursor"] = if next < entries.len() {
        json!(format!(
            "volumes-v1:{}",
            entries[next - 1]["mount_path_bytes_hex"].as_str().unwrap()
        ))
    } else {
        Value::Null
    };
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_local_volume_inventory_has_no_invented_shared_total() {
        let answer = volumes(2, None).unwrap();
        assert!(answer["records"].as_array().unwrap().len() <= 2);
        assert!(serde_json::to_vec(&answer).unwrap().len() <= crate::OUTPUT_BUDGET);
        for entry in answer["records"].as_array().unwrap() {
            assert_eq!(entry["local"], true);
            assert_eq!(entry["free_space_sum_allowed"], false);
            assert!(entry["container_id"].is_null());
        }
    }

    #[test]
    fn native_startup_and_data_uuids_are_not_conflated() {
        if Path::new("/System/Volumes/Data").exists() {
            let system = native_uuid(Path::new("/")).unwrap();
            let data = native_uuid(Path::new("/System/Volumes/Data")).unwrap();
            assert!(system.is_some());
            assert!(data.is_some());
            assert_ne!(system, data);
        }
    }

    #[test]
    fn malformed_volume_attribute_buffers_are_bounded() {
        for len in 0..80 {
            let _ = parse_uuid(&vec![0xff; len]);
        }
        let mut seed = 7_u64;
        for _ in 0..10_000 {
            let mut bytes = [0_u8; 64];
            for b in &mut bytes {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                *b = (seed >> 32) as u8;
            }
            let _ = parse_uuid(&bytes);
        }
    }
}

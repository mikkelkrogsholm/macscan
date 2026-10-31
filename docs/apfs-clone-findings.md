# APFS clone metadata probe (#7)

Measured 2026-10-31 on Apple Silicon, macOS 26.6.2 (25G83), SDK 26.5,
Rust 1.99.1. The example is explicitly opt-in and is never called by scanning.
It changes no engine metrics, snapshot schema, analysis, CLI or UI behavior.

## API evidence

Apple's public [attr.h](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/sys/attr.h),
[stat.h](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/sys/stat.h),
[getattrlist(2)](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/man/man2/getattrlist.2),
and [getattrlistbulk(2)](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/man/man2/getattrlistbulk.2)
are primary references; the installed SDK headers/manpages were also checked.

- `ATTR_CMNEXT_CLONEID` and `ATTR_CMNEXT_EXT_FLAGS` return `u_int64_t`.
  Extended common attributes occupy `forkattr` with `FSOPT_ATTR_CMN_EXTENDED`.
  Clone ID identifies the data stream; pure clones have matching IDs.
- `ATTR_FILE_ALLOCSIZE` covers all forks; `ATTR_FILE_DATAALLOCSIZE` covers
  the DATA fork. Both are `off_t`. These are allocation observations, not
  exclusive/private or reclaimable bytes.
- `EF_MAY_SHARE_BLOCKS` (1) is not proof of full sharing.
  `EF_SHARES_ALL_BLOCKS` (64) indicates full sharing, and implies may-share.
  The resource-fork experiment below shows why this cannot justify removing
  the entire file's reported allocation.
- Returned attribute bits control the packed layout. This probe does not
  request `FSOPT_PACK_INVAL_ATTRS`; absent fields consume no bytes. It validates
  bounds, returned masks, required name, signed allocation validity and
  DATA <= total; malformed records fail explicitly.
- Volume capabilities have separate valid and capability masks.
  `VOL_CAP_INT_CLONE` means clone operations are supported;
  `VOL_CAP_FMT_CLONE_MAPPING` means full clone tracking is supported.
  Here clone support was valid/true, clone mapping valid/**false**, while
  returned full-clone IDs/flags still worked. This does not certify full tracking.

[clonefile(2)](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/man/man2/clonefile.2)
requires source/destination on the same filesystem (`EXDEV` otherwise).
Together with device identity this supports a conservative `(device, cloneID)`
grouping boundary. Actual clone-ID collisions or clone behavior on a second
native volume were **not tested**. A synthetic same-ID/different-device test
proves the accounting helper keeps those observations separate. No volumes or
disk images were created or mounted.

Supporting reference: Dua pinned at
[`bc03feddbdd946c16dd234a28b377d43c0688597`](https://github.com/Byron/dua-cli/tree/bc03feddbdd946c16dd234a28b377d43c0688597),
its [attribute parser](https://github.com/Byron/dua-cli/blob/bc03feddbdd946c16dd234a28b377d43c0688597/crates/dua-lib/src/macos/attributes.rs)
and [tests](https://github.com/Byron/dua-cli/blob/bc03feddbdd946c16dd234a28b377d43c0688597/crates/dua-lib/src/macos/tests.rs)
cover returned masks, clones, hardlinks and independent resource-fork allocation.
Dua's identity accessor gates on may-share; this experiment deliberately requires
shares-all for its supplementary correction. Dua is MIT licensed. No Dua
implementation was copied and no dependency was added.

## Observations

Only a private `mkdtemp` tree was used. `clonefile` creates clones explicitly;
errors never fall back to byte copying. Three clones start from a 1 MiB source;
one clone receives a 4 KiB overwrite, another an independently written 64 KiB
resource fork. A hardlink points to the source. IDs below are symbolic; no
user paths, device numbers or persistent identifiers are recorded here.

| Fixture | Inode relation | Clone ID relation | Extended flags | Total allocation | DATA allocation |
|---|---|---|---:|---:|---:|
| source | source inode | source ID | 65 | 1,048,576 | 1,048,576 |
| hardlink | same as source | source ID | 65 | 1,048,576 | 1,048,576 |
| full | distinct | source ID | 65 | 1,048,576 | 1,048,576 |
| forked | distinct | source ID | 65 | 1,114,112 | 1,048,576 |
| partial | distinct | different ID | 1 | 1,048,576 | 1,048,576 |
| sparse | distinct | different ID | 16 | 4,096 | 4,096 |

All six total allocations matched `stat.blocks() * 512`. Sparse logical size
was 16,777,216 bytes. To make the hole unambiguous, the fixture explicitly uses
`F_PUNCHHOLE` on its own file. Earlier length-extension/distant-write variants
reported full allocation (one had the sparse flag); neither a sparse flag nor
an assumed file-creation strategy proves allocated-byte savings.

Raw fixture sum: **5,312,512** bytes. Hardlink-accounted sum: **4,263,936**.
Experimental full-DATA-clone-adjusted sum: **2,166,784**, comprising one shared
1 MiB DATA observation, the partial clone's full 1 MiB observation, the independent
64 KiB resource fork, and 4 KiB sparse allocation. Hardlinks are counted once
before clone grouping. Non-data allocation survives the full-DATA correction.
The probe fails if native full/partial clone or independent resource-fork
invariants are not observed.

Thus these fixtures support leaving raw and hardlink metrics unchanged and
computing a **separate, limited** full-DATA-clone observation if desired by a
future product decision. Only nonzero returned IDs with known device, known
DATA allocation and shares-all can participate. Missing IDs, fields or flags
retain full allocation. Partial sharing retains full allocation even when a
synthetic test supplies a related/equal clone ID. This is not full unique APFS
tracking, a reclaimability estimate, or an estimate of partial sharing. The
volume capability and live mutable filesystem leave those limits explicit.

## Small timing result

After fixture observations, 994 tiny files bring the directory to exactly
1,000 entries. Both requests warm once; 12 measured trials alternate order.
Both request name/device/inode/total/DATA; the extended request adds only
clone ID/extended flags plus its required option. Each trial opens a fresh
directory descriptor and enumerates/parses the same metadata in 64 KiB buffers.

| Request | Entries per trial | Trials | Minimum ms | Median ms | Maximum ms |
|---|---:|---:|---:|---:|---:|
| baseline | 1,000 | 12 | 2.030 | 2.153 | 2.776 |
| extended | 1,000 | 12 | 2.038 | 2.237 | 2.847 |

Extended/baseline median ratio was **1.039** (about 3.9% slower) in the retained
run. This warm-cache debug-build microprobe is noisy, uses tiny synthetic files,
and excludes scan finalization/UI/Parquet. It does not establish whole-disk
performance, a million-file result, or an acceptable product performance budget.
No cache purge or system settings changes were performed.

## Reproduction and verification

From a clean checkout with the configured Rust toolchain/dependencies cached:

```sh
probe_toolchain="$HOME/.rustup/toolchains/1.99.1-aarch64-apple-darwin/bin"
export RUSTC="$probe_toolchain/rustc" RUSTDOC="$probe_toolchain/rustdoc"
export RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER=
export CARGO_TARGET_DIR="$PWD/.build" CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0
export MACOSX_DEPLOYMENT_TARGET=14.0
"$probe_toolchain/cargo" test -p macscan-engine --example apfs_clone_probe --locked --offline
MACSCAN_APFS_SYNTHETIC_PROBE=1 "$probe_toolchain/cargo" run -p macscan-engine --example apfs_clone_probe --locked --offline
```

Nine tests passed: malformed/truncated/unrequested fields, missing/zero identity,
negative allocation, returned-field layout, hardlinks and resource remainder,
partial/unknown sharing, missing DATA/device, and cross-device grouping.
Simulated `EINVAL`/`ENOTSUP` retries baseline metadata without clone grouping;
other syscall errors remain errors. Missing attributes remain unknown and never
become a grouping key. Native unsupported filesystems were not tested. If clone
creation itself is unsupported the probe fails explicitly; its metadata fallback
does not substitute copied files. If extended metadata is unsupported its timing
trials are skipped. Missing essential total allocation fails the diagnostic.

The example compiles with deployment target 14.0; `otool -l` reports `minos 14.0`
and SDK 26.5. **macOS 14 runtime behavior was not tested**. No full-platform,
provider, restricted-root or GUI certification is claimed.

The native run and an explicit SIGTERM test both removed their known private
trees; SIGINT/SIGTERM set a flag and normal error unwinding performs cleanup.
Uncatchable termination cannot promise cleanup. Without opt-in the example
exits before creating a tree. Private local evidence is ignored under
`evidence/issue-7/`: `run.json`, `run.log`, `tests.log`, `stop.log`, `opt-in.log`
and reference downloads. JSON contains fixture metadata and per-trial elapsed
nanoseconds, not scanned user-file content. All verification processes finished.

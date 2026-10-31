# Provider metadata clarification (#11)

This is a standalone clarification, not a change to scanning policy. The current
product's metadata-only intent does **not** establish a no-hydration guarantee.
The absence of an explicit guard does not show that hydration actually happened.
No real provider, placeholder, user-selected directory or user file was accessed
for this experiment. No files were marked SF_DATALESS. Product go/no-go remains
MAIN's decision; the evidence below separates demonstrated control flow from
unknown provider behavior.

## Primary contract and interpretation

Reviewed 2026-10-31:

- [Apple TN3150: Getting ready for dataless files](https://developer.apple.com/documentation/technotes/tn3150-getting-ready-for-data-less-files)
  ([primary Markdown](https://docs.developer.apple.com/tutorials/data/documentation/technotes/tn3150-getting-ready-for-data-less-files.md)).
  Directory enumeration and existence checks may materialize data. Apple's
  example trace includes `getattrlistbulk`. `stat` and `getattrlist` can
  materialize intermediate dataless folders. Absence of SF_DATALESS does not
  establish local I/O; Apple explicitly gives NFS as a counterexample.
- Installed Apple SDK `usr/include/sys/resource.h` and `man getiopolicy_np(3)`:
  `getiopolicy_np(int iotype, int scope)` returns a policy or -1/errno;
  `setiopolicy_np(int iotype, int scope, int policy)` returns 0 or -1/errno.
  The materialization type is 3, THREAD scope 1, DEFAULT 0, OFF 1, ON 2.
  Only THREAD is used here. The man page describes OFF as disabling dataless
  materialization by the current thread, and thread DEFAULT as following the
  process policy; process DEFAULT follows the system default (OFF, according to
  this installed man page). Restore the **observed original**, not an assumed DEFAULT.

TN3150's prose calls OFF an iotype; its C sample and the installed header/man
agree that `IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES` is the iotype and OFF is
the third argument. The probe follows that actual call contract. `EDEADLK`
means an operation was blocked and coverage is unknown, never an empty directory
or a zero-size file. OFF is not evidence that provider contact, networking,
permission checks or synchronous blocking are impossible.

## Current production call audit

Audit base: `01a17121ee8b326d9600c6a764c23463fc688361` (the clarification commit
adds only this document and the example). These are code observations; risk
statements about the APIs derive from TN3150, not a provider experiment.

| Phase / code | Present gate and calls | Remaining contact/materialization/blocking risk |
| --- | --- | --- |
| Root, `walk.rs::walk_with_progress_guarded` | Validates options/cancellation, absolutizes path; `symlink_metadata` happens **before** cloud/dataless/exclusion gating. Rejects symlink roots. | Initial path resolution/metadata can contact a provider or traverse dataless ancestors before flags are known. No policy guard. |
| Root resolution | Known `cloud_path` or dataless root skips `canonicalize`; otherwise canonicalizes, then checks the resolved path again. | An unknown provider or dataless ancestor can be reached during canonicalization. The two hard-coded path substrings (`Library/CloudStorage`, `Library/Mobile Documents`) are not provider discovery. |
| Root open, `platform.rs::Directory::root` | After root gates, `open(O_RDONLY\|O_DIRECTORY\|O_CLOEXEC\|O_NOFOLLOW_ANY)`, `fdopendir`, `fstat`; compares device/inode with initial metadata. | Symlink protections and descriptor identity prevent some races; they do not suppress dataless materialization or establish provider locality. A directory open/path lookup may block. |
| Root capacity, `walk.rs`, `volume.rs` | Opens `.` from admitted root (or parent for a non-directory root), `fstatfs`. Supported-local check follows this open/capture. For supported local mount roots, Foundation capacity/UUID resources and `getattrlist(ATTR_VOL_UUID, FSOPT_NOFOLLOW_ANY)` run. | Excluded roots do not reach capture. The mount-local check is too late to protect earlier root calls. Capacity is volume scope, not evidence about residency of a provider subtree. Foundation and mount metadata add synchronous work; mount-root restriction is not a measured no-contact guarantee. |
| Progress discovery, `walk.rs::Branches::discover` | Only when admitted traversal has a stack; checks Stop/duration, opens root again, calls `next_metadata` at most 4096 times, checking between calls. | Duplicate enumeration has the same risk as traversal. Limits bound subsequent calls, not an in-flight synchronous syscall. |
| Bulk traversal, `bulk.rs`, `platform.rs::next_metadata_event` | 64 KiB `getattrlistbulk` batches request names, flags, identity and sizes. Fully returned regular-file records avoid per-entry stat. Directories, symlinks, other kinds or missing attributes fall back to `fstatat(AT_SYMLINK_NOFOLLOW)`. | Bulk enumeration itself may materialize the directory. A returned dataless flag is learned after that operation. Metadata fallbacks can contact providers. No materialization policy guard exists. |
| Enumeration fallback | Only initial unused-bulk EINVAL/ENOTSUP/ENOSYS triggers `rewinddir`, `readdir`, then `fstatat`; other errors are recorded, not retried through fallback. | Changing APIs is not a no-hydration safeguard. Under a future OFF guard, EDEADLK must remain a coverage failure, with no permissive retry. |
| Child open, `walk.rs`, `platform.rs::child` | Metadata is emitted for files without opening their contents. Directories are excluded for device change, explicit path, dataless/cloud path, or depth. Otherwise `openat(O_DIRECTORY\|O_NOFOLLOW)` + `fstat` verifies child identity. | Dataless directory is not traversed **after recognition**. Recognition required prior enumeration/stat. Unknown providers and races after metadata remain; descriptor-relative access does not supply materialization policy. |
| Final reads | `walk.rs` repeats capacity capture (`fstatfs`, conditional mount-root resources/UUID) and checks fsid before/after. Opened root/child identity checks are `fstat` at open time. | There is no final full-tree residency/identity revalidation. Volume identity is not proof of no provider contact or stable residency. Final reads also need any future guard. |
| Legacy `scan.rs::scan_with_progress` | Same initial metadata/canonicalization/root/cloud/capacity gates; `readdir` + `fstatat`, then child gates/open; final capacity. File-root parent gets a device check. | Same pre-gate and metadata risks; no progress discovery/bulk path here. It also has no materialization guard. |
| Saved-scan consumers | Analysis streaming calls `walk_with_progress_guarded`; focused remeasurement sets `require_saved_destination` to reject canonical destination changes after the provider gate. Existing comparison checks saved root/OS/volume/policy and per-path coverage. | Saved-data queries do not establish current provider state. Focused destination validation is not a no-hydration guard; remeasurement still invokes live root calls above. Do not reuse a policy fingerprint if broad provider exclusions become partial observations. |

The existing local filesystem allowlist rejects known unsupported volume results;
it cannot infer that a provider-backed directory on APFS is fully local. None of
these gates turns the present product into a guarded provider implementation.

## Probe and synthetic classification

`crates/engine/examples/provider_metadata_probe.rs` accepts exactly
`--owned-local-fixture`, with no path option. Default/other arguments return before
fixture creation. The opted-in run creates one unique `/private/tmp` directory
and its own 33-byte file/empty child. It confirms MNT_LOCAL on the fixture before
writing. It never reads file contents. Cleanup removes only the three known owned
paths, with no recursive traversal. No provider login, download, settings, privacy
or process-wide policy change occurs.

The dedicated thread gets its original policy, sets OFF, then queries to confirm
OFF **before any fixture operations**. An unsupported get/set or failed/mismatched
confirmation fails closed. After an operation error, cancellation or caught unwind,
it restores the exact original and queries to verify restoration. A failed setup
set also attempts restoration. Restoration failure is reported and fails the run;
the dedicated thread terminates rather than returning to a reusable worker pool.
A process abort, SIGKILL or permanently stuck syscall cannot run Rust cleanup.
SIGINT/SIGTERM request Stop; they do not preempt a syscall. Cleanup and policy
restoration still run after Stop, outside the normal next-call budget.

The native operation sequence is bounded to 32 high-level calls and a five-second
between-call budget: create/open owned fixture; fstatfs; write synthetic data;
create child; symlink_metadata; canonicalize; fstat; one 4096-byte getattrlistbulk;
read_dir plus up to budgeted iterator calls; two fstatat calls; openat/close child;
final fstat; close handles and explicit owned cleanup. Standard-library wrappers
may issue more than one syscall. This probe does not execute production's entire
attribute set, mount-root Foundation queries or traversal implementation.

Eight injected example tests cover unsupported get/set, failed OFF confirmation,
EDEADLK, access denied, cancellation before the next operation, operation unwind,
restore set/get/mismatch failures and the matrix below. These prove control flow,
not any native provider behavior. The `classify`/`comparable` helpers are compiled
only for tests. Their decisions are **hypothetical inputs** for a later confirmed
OFF guard and independently verified provider scope, not production readiness.

| Synthetic input | Logical | Allocated | Hypothetical coverage / descent |
| --- | --- | --- | --- |
| Dataless provider directory placeholder | Known 42 | Unknown (`null`) | Partial; never descend |
| Known local provider file | Known 42 | Known 4096 | Observed file; no directory descent |
| Unknown/unavailable provider root | Unknown (`null`) | Unknown (`null`) | Partial; never descend |
| Ordinary local nonprovider directory | Known 0 own bytes | Known 0 own bytes | Eligible for guarded bounded descent; subtree completeness still requires successful enumeration |

A known zero is distinct from unavailable. Logical and allocated fields remain
independent; neither is a claim about private/reclaimable bytes. The synthetic
comparison rejects broad-excluded-v1 versus partly-observed-off-v2 fingerprints,
and rejects incomplete coverage even with equal fingerprints. Therefore a missing
row in either case is unknown, not a deletion or a zero delta. Any eventual
production change must encode the new policy/coverage semantics in saved identity;
this example intentionally does not change existing stores or comparison code.

## Later provider acceptance protocol (requires explicit authorization)

No live provider run is authorized or implemented here. A later run must name one
user-chosen disposable provider area, provider/version/account mode, exact macOS
version and bounded entry/depth limits. Do not create, evict, mark or hydrate
placeholders to manufacture a passing test. Do not enumerate an account home.

1. Establish independent before-state evidence from the provider's supported
   residency/status tooling or UI without content/preview access, plus provider
   diagnostics and a bounded network observation window. Record which tools can
   themselves contact the provider. Include an unobserved control window to help
   distinguish background sync. If before-state is unavailable or ambiguous,
   report unknown; do not infer residency from allocated bytes or SF_DATALESS alone.
2. Predeclare a maximum of one root metadata operation, one directory open/fstat,
   one bounded enumeration batch (at most 10 direct entries), metadata for at most
   those 10 entries, no child descent, and one final identity read. Include every
   path-resolution, progress and capacity side call in the plan: disable progress
   discovery and mount-resource queries for the first bounded experiment. Exact
   APIs/attributes must be frozen before the run. No file-content opens, reads,
   checksums, Quick Look, forced hydration, download or retries with policy ON.
3. Run all target operations, including initial root metadata and final identity,
   on a dedicated thread with original -> OFF -> confirmed OFF. Fail closed on
   unsupported/setup failure. Record each operation/time/errno. EDEADLK means
   materialization blocked; access denied, offline, unavailable attributes and
   timeout are separate partial/unknown coverage states. Never convert any into
   an empty directory, zero bytes, complete subtree or deletion.
4. Check Stop before each planned call; use a five-second between-call budget and
   the fixed entry limit. An AtomicBool cannot interrupt synchronous VFS/provider
   I/O. For a hard wall-clock containment requirement use a separately supervised
   helper process, and treat its forced termination as inconclusive with possible
   incomplete cleanup. Do not start another attempt while an earlier job is live.
   Never downgrade OFF or switch APIs to bypass a blocked materialization.
5. Verify exact restoration on success/error/Stop/unwind; dispose of the thread on
   failure. Compare independent after-state/provider diagnostics/network evidence
   against before-state and control window. A changed residency state or observed
   download fails no-hydration acceptance. Quiet network alone cannot prove it;
   unobservable state/contact is unknown. Archive sanitized evidence, provider/OS
   versions, policy fingerprint, limits and all uncovered operations. Approval of
   one bounded situation does not authorize recursive scanning or another provider.

## Evidence and readiness by situation

| Situation / provider | Evidence as of 2026-10-31 | Readiness boundary |
| --- | --- | --- |
| Owned ordinary local fixture, macOS 26.6.2 (25G83), arm64 | Actual OFF query and restore succeeded; 17 calls, two entries by bulk and readdir, stable inode/device, own file logical 33 / allocated 4096; fixture removed. Caller policy 0 before/after. | Local API/control-flow experiment passed. Not provider or product certification. |
| Current product on ordinary local disks | Audit only in this clarification; existing product tests are outside this run. | No new behavior or safety claim delivered. |
| iCloud Drive / Mobile Documents | Known-path production exclusion; no live evidence. | Bounded no-hydration acceptance unknown; current root pre-gate calls still exist. |
| File Provider domains under CloudStorage (including third-party providers) | Known-path exclusion, provider/version-specific behavior untested. | Unknown per provider/version; cannot inherit certification from iCloud or local APFS. |
| Unknown provider path, offline/unavailable provider, or remote/NFS volume | No native/provider experiment. No flag/locality inference is valid. | Fail closed/unknown in a proposed guarded path; current product cannot be certified from this result. |
| Product minimum macOS 14 | Compiled/linked with deployment target 14.0 using installed SDK; no macOS 14 runtime used. | Actual get/set/OFF semantics, errors and provider behavior on 14 remain unverified. Header symbol availability is not runtime acceptance. |
| Restoration/setup failure | Injected tests only; no such native failure observed. | No target operations on setup failure; report failure and terminate dedicated thread on restore failure. No permissive fallback. |

These are evidence states, not a blanket no-go decision to close the issue. MAIN
can decide the next product boundary with explicit unknowns retained.

## Reproduce the authorized local evidence

Ignored local records live in `evidence/issue-11/{tests,default,native}.log`, the
primary `tn3150.md`, and `verification.log`. Initial compilation exposed missing
libc Rust declarations; the example now declares the two public C scalar APIs
locally, with no dependency or Cargo.lock changes. All reported passing commands
below ran after that correction:

```sh
export PATH=/Users/mikkelfreltoftkrogsholm/.rustup/toolchains/1.99.1-aarch64-apple-darwin/bin:/usr/bin:/bin
export RUSTC=/Users/mikkelfreltoftkrogsholm/.rustup/toolchains/1.99.1-aarch64-apple-darwin/bin/rustc
export RUSTDOC=/Users/mikkelfreltoftkrogsholm/.rustup/toolchains/1.99.1-aarch64-apple-darwin/bin/rustdoc
export CARGO_TARGET_DIR="$PWD/.build" CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 MACOSX_DEPLOYMENT_TARGET=14.0
cargo test --locked --offline -p macscan-engine --example provider_metadata_probe
cargo run --locked --offline -p macscan-engine --example provider_metadata_probe
cargo run --locked --offline -p macscan-engine --example provider_metadata_probe -- --owned-local-fixture
git diff --check
```

Results: 8/8 tests passed; default exited without creating a fixture; native exited
0 with original=0, confirmed_off=true, restoration observed=0/verified and caller
unchanged. Sanitized native report: `logical=33`, `allocated=4096`,
`bulk_entries=2`, `readdir_entries=2`, `identity_stable=true`, `bounded_calls=17`.
The exact emitted unique fixture path was checked absent after cleanup. No full
desktop build, app launch, live-provider access or minimum-OS runtime test ran.

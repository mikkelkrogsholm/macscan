# Optional raw APFS metadata capture (#8, run A)

Streaming callers may explicitly set `ScanOptions.capture_clones = true`.
The default is false, including existing CLI and desktop callers. Legacy
in-memory `scan`/`scan_with_progress` reject true with `Unsupported`; they do
not silently ignore it. No user CLI flag, UI, derived clone metric, automatic
enrichment or accounting change is included in this run.

The supplemental contract is version 1 (`CLONE_CAPTURE_VERSION`), independent
of traversal policy and the existing Parquet format/schema version. Capture on
and off therefore retain the same primary comparison policy. Logical, raw
allocation, hardlink accounting, coverage and classification keep their existing
meaning. Neither clone ID nor flags alone establishes unique/private or
reclaimable bytes; partial sharing is not inferred. See the limits and Apple
primary references in [the probe findings](apfs-clone-findings.md).

## Capture boundary and request

Only admitted traversal directories enable the extended bulk request, after
existing root/child cloud, dataless, exclusion and volume gates. An admitted
parent's existing metadata batch necessarily returns fields for child names
before their individual path gates; excluded, dataless, provider and foreign
device child observations are discarded. No extra traversal or per-file clone
call is performed. Nonregular entries have unavailable supplemental metadata.
The existing root pre-gate metadata/canonicalization limitation documented for
#11 remains; this feature does not certify provider safety or no hydration.

Opt-in adds public `ATTR_CMNEXT_CLONEID` and `ATTR_CMNEXT_EXT_FLAGS` in
`forkattr`, `FSOPT_ATTR_CMN_EXTENDED`, and `ATTR_FILE_DATAALLOCSIZE` to the
existing 64 KiB bulk request. Returned masks determine layout; unreturned
attributes do not occupy space. DATA and total allocation are signed `off_t`;
negative allocation or DATA greater than total cannot be a valid observation.
An invalid supplemental tail retains coherent primary metadata, or uses the
existing `fstatat` fallback when primary fields are unavailable. A rejected
initial extended call uses the existing rewind/readdir/stat fallback; a consumed
cursor or malformed returned batch is never permissively retried. Optional
unavailability alone does not invalidate primary coverage or classification.

Capture-off uses exactly the previous base request and makes no clone calls or
capability queries. Volume clone capabilities are not queried in this run.
The developer prefetch experiment remains off by default and retains its
single-worker, owned-descriptor, 64 KiB bounds. Extended capture batches run
serially; the base prefetch request and parser remain shared with normal scans.

## Persisted field contract

When capture is requested, these nullable supplemental fields survive streaming
raw batches, final Parquet, `entries_saved`, bounded SQL and `inspect`:

| Column | Type | Meaning |
|---|---|---|
| `clone_status` | VARCHAR | Fixed status below; NULL for historical absent column |
| `clone_id` | UBIGINT | Raw returned ID, including a known zero |
| `clone_extended_flags` | UBIGINT | Raw returned extended flags |
| `clone_data_allocated_bytes` | UBIGINT | Returned DATA allocation, separate from all-fork allocation |
| `clone_device_id` | UBIGINT | Device from the same returned record, never synthesized from fallback stat |

`not_requested` means capture was off. `observed` means all supplemental fields
and complete coherent bulk primary metadata were returned and validated; a known zero ID
remains observed and is not an eligible grouping key. `missing` means required
fields were absent, the file was a root/fallback observation without safe capture,
or supplemental metadata was discarded at a scope gate. `unsupported` records
an initial extended bulk rejection. `invalid` records invalid supplemental
values/layout; its raw optional fields are discarded conservatively. No absent
attribute is converted to zero. Partial missing observations may retain the
other returned fields. Nonregular rows may use `missing` or `not_requested`;
these are outside the regular-file capture counter universe.

Manifest `context.clone_capture` is assembled before atomic publication:
`version`, `requested`, `scope = admitted_directory_bulk_regular_files`,
`capability_status = not_queried`, `counts_collected`, and
`regular_file_status_counts`. With capture requested, counts cover
emitted regular-file rows, including unavailable root/file and discarded child
observations; directories are not missing clone coverage. With capture off,
`counts_collected = false` and the counter object is empty (uncollected, not a
measured zero). No per-file supplemental JSON counting runs, and both physical
raw and final Parquet retain the base schema without clone columns. At read time,
explicit saved `requested = false` synthesizes `clone_status = not_requested`
and NULL raw clone fields. A historical absent capture context keeps all absent
clone columns NULL. Legacy default Parquet saves also omit unrequested columns;
explicitly present supplemental observations remain preserved. No post-publication
manifest mutation is needed. Historical snapshots with absent capture context attach with NULL supplemental
columns and no rewriting. Missing clone/mtime columns do not disable saved
classification context. Legacy JSONL accepts old 15-element rows and new rows
with an optional supplemental object as element 16 (default saves retain the
15-element shape), preserving raw fields when
saved through legacy storage. SQL JSON results preserve UBIGINT as decimal
strings, consistent with existing byte/identity fields.

## Verification and reproduction

Only private synthetic fixture trees are enumerated. The native test uses
`clonefile` with no copied-byte fallback, a hardlink, a changed clone, an
independent resource fork, and `F_PUNCHHOLE` on its own sparse file. It exercises
actual production `scan_to_store` and reads metadata back from Parquet. Catchable
SIGINT/SIGTERM request cancellation; normal unwinding removes owned fixtures.
Uncatchable termination cannot guarantee cleanup.

```sh
capture_toolchain="$HOME/.rustup/toolchains/1.99.1-aarch64-apple-darwin/bin"
export RUSTC="$capture_toolchain/rustc" RUSTDOC="$capture_toolchain/rustdoc"
export RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER=
export CARGO_TARGET_DIR="$PWD/.build" CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0
export MACOSX_DEPLOYMENT_TARGET=14.0
"$capture_toolchain/cargo" test -p macscan-engine clone_tests --locked --offline
"$capture_toolchain/cargo" test -p macscan-analysis clone_tests --locked --offline
"$capture_toolchain/cargo" test -p macscan-analysis --test clone_capture --locked --offline
"$capture_toolchain/cargo" test -p macscan-analysis --test clone_capture native_owned_apfs_clone_capture_roundtrip --locked --offline -- --ignored --exact --nocapture
scripts/build-local.sh
```

On macOS 26.6.2, all six fixture files returned observed supplemental fields
after Parquet. Source/full/hardlink/forked retained equal IDs with flags 65;
the changed clone had a different ID and flags 1. The independent fork retained
65,536 bytes beyond DATA allocation; sparse allocation was 4,096 bytes for a
16 MiB logical file. Capture-off/on primary rows, hardlink owners, coverage,
classification and policy were equal; primary `Store.compare` was empty.
No full clone-mapping capability, other native volume or macOS 14 runtime was
certified. Deployment target 14.0 is a compilation check only. Unsupported
native clone creation or extended metadata is reported explicitly, never faked.
No user/provider scan, cache purge or application launch is required.

Focused tests cover returned masks, zero/missing IDs, missing flags/DATA,
truncation, negative/oversized DATA, negative total fallback, cross-device raw
identity, unsupported initial fallback, root/child exclusion, root-file
unavailability, legacy/Parquet roundtrip and historical optional-column attach.
Cancellation and exhausted budgets preserve snapshots and remove unpublished
stages. Private command logs and the sanitized native matrix are retained under
ignored `evidence/issue-8A/`. Run A leaves #8 open for later consumer work.

The A2 correction removes unrequested per-file counters and physical clone
columns from the capture-off production path. A controlled, same-window owned
100,000-file benchmark retained an executable built from pre-capture revision
`31a5535`, then ran it and the corrected executable sequentially with the same
pinned toolchain, fixture shape, warmups and three measured repeats per mode.
All twelve measured baseline/fallback runs compared 100,089 paths with zero
mismatches and removed their owned trees. Baseline median total was 822.052 ms
for the reference and 630.652 ms for the correction, within the 10% acceptance
bound. The existing watcher join strongly quantized wall time (median 196.181 ms
versus 8.878 ms); these totals do not establish a speedup. Median walk plus all
flush work was 433.393 ms versus 429.981 ms (ratio 0.992), supporting near-original
default-path cost without hiding work behind the watcher. These are warm,
three-repeat synthetic observations, not a cold-cache or whole-device claim.
The watcher and aggregation algorithms were unchanged. Reports and executable
hashes are retained in ignored `evidence/issue-8A2/`.

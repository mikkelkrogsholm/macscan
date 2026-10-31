# Limited APFS DATA-clone metric

`apfs-data-clone` is the separate metric **Begrænset DATA-klonkorrigeret
allokering**, algorithm `apfs-data-clone-1`, capture contract version 1.
Logical, raw allocation, hardlink-accounted allocation and `private_bytes`
retain their meanings; private bytes remain unavailable.

Capture is explicitly opt-in on a new streaming measurement:

```sh
macscan scan /synthetic/tree --capture-apfs-clones --metric apfs-data-clone \
  --output "$HOME/macscan-owned-history"
macscan summary --scan ID --metric apfs-data-clone --store "$HOME/macscan-owned-history"
macscan largest --scan ID --metric apfs-data-clone --store "$HOME/macscan-owned-history"
macscan compare BEFORE_ID AFTER_ID --metric apfs-data-clone --store "$HOME/macscan-owned-history"
```

`scan --metric apfs-data-clone` requires explicit `--capture-apfs-clones`
before any traversal or publication. The default remains capture off. Summary,
children, largest, inspect, sunburst, insights, file profile, comparison and
growth all use the same saved metric. In-memory engine query/comparison reject
it explicitly. Desktop exposes the saved metric and explicit new-scan capture
controls as described below. Building the app does not launch or activate it.

## Accounting and limits

Finalization uses only saved bulk metadata, disk-backed SQL, the existing
hardlink ownership and prefix/subtree pipeline. No new filesystem calls,
content reads or second traversal are involved. Capture off retains the base
physical raw and final Parquet schemas, with no supplemental aggregation.

Hardlinks are charged first to the existing deterministic raw-path owner.
Contradictory or unavailable same-inode alias observations disqualify that
inode from supplemental reduction. Only ordinary hardlink-owner files with
coherent observed metadata, a nonzero clone ID, `SHARES_ALL_BLOCKS` (64),
known same-record device matching the measurement device, and valid DATA
allocation not exceeding all-fork allocation enter `(device, clone ID)` groups.
A group must agree on DATA allocation and logical size. Inconsistent groups
retain their hardlink-accounted allocations and expose fallback quality.

The raw-path-byte minimum eligible file owns a group's DATA allocation.
Other eligible files subtract only DATA allocation, retaining each inode's
independent non-DATA/resource-fork allocation. Hardlink aliases stay zero.
Nonfiles retain their own allocation. Zero IDs, known nonfull or partial
sharing never reduce allocation. No partial-sharing estimate is attempted.

Ownership is global within the measurement root. An owner outside a selected
directory, search or category can leave a visible file with zero DATA charge.
This is attribution of a limited observation, not empty file content, unique
space, private space, reclaimable space or deletion advice. A live scan is
non-atomic; volume clone mapping capability is not certified.

## Saved and response contract

Capture-on final Parquet adds these fields alongside the raw capture fields:

| Field | Meaning |
|---|---|
| `apfs_data_clone_bytes` | Complete subtree/file limited allocation; NULL when primary coverage is incomplete |
| `observed_apfs_data_clone_bytes` | Observed subtotal, including retained fallback allocation |
| `own_apfs_data_clone_bytes` | Own contribution after hardlink and limited DATA correction |
| `apfs_data_clone_owner` | Eligible group owner entry ID, including self; NULL when unavailable/ineligible |
| `apfs_data_clone_reason` | Saved contribution/fallback reason |

Byte values and IDs in JSON are exact decimal strings, including values above
2^53. `charged_to` and all primary fields remain unchanged. Directory sums
include directory own allocation. Unknown totals remain NULL; observed
subtotals are not complete totals.

Manifest and shared envelopes contain `apfs_data_clone` with algorithm and
capture versions, common label, limitations, `status` (`limited` or `fallback`),
`comparison_ready`, `reason_counts` and
`unavailable_or_invalid_observations`. Counts cover emitted regular-file rows,
including hardlink aliases; directories are outside that count universe.
`comparison_ready` describes supplemental metadata only; comparison additionally
requires complete primary coverage. Per-row reasons distinguish full DATA
owners/nonowners, hardlink nonowners, limited retained observations and
fallbacks. Sunburst includes owner/reason for this metric; inspect exposes them
as saved metadata. Native history exposes the saved quality tag for later UI
baseline filtering.

Unavailable/invalid metadata and unknown/non-APFS filesystems conservatively
retain hardlink-accounted allocation with a visible fallback status. Global
volume invalidation makes complete totals unknown and uses the primary
hardlink-accounted observed subtotal as fallback, without certifying previously
saved clone reductions. Primary coverage is not weakened by supplemental
metadata unavailability.

Historical, uncaptured, raw-capture-only or unknown-algorithm measurements
remain readable for primary metrics. This metric requires a new compatible
measurement; no automatic enrichment or history rewrite occurs. Comparisons
require matching supported capture/algorithm, root, traversal policy, OS and
stable volume identity, complete primary coverage, all regular metadata
observed and no invalid/inconsistent observations. Fallback/incomplete pairs
are rejected with guidance to use logical or allocated metrics.

Clone comparison/growth detect DATA charge changes even when primary sizes are
unchanged. Ordinary SQL `changes` and primary compare/growth retain their
primary-size relevance semantics; a clone-only change does not add zero-primary
rows. SQL can explicitly select the saved supplemental columns. Agent responses
retain the existing 8 KiB bound, cursors and exact integer representation.

## Native desktop

The short third metric button is **APFS DATA**; its accessible name, chart note
and consumer status use **Begrænset DATA-klonkorrigeret allokering**. It applies
to saved file measurements, children/search, chart, inspector, insights, file
profiles and growth. Whole-Mac capacity remains the macOS volume/container
view, with all file metric buttons disabled.

The **APFS-klonmetadata til næste nye scanning** checkbox starts unchecked and
applies to the next folder, volume or whole-Mac scan only. It resets after a
successful start. The existing exact-entry rescan confirmation has its own
unchecked **APFS-klonmetadata til denne nye måling** checkbox. It always confirms
the already selected saved root, directory or file; no automatic capture or
historical enrichment occurs. Both controls feed the existing single-job
launcher; the optional native `captureClones` argument defaults to false for
older callers, and whole-Mac execution passes it to each planned volume.

Old, uncaptured, raw-only or unknown-version measurements keep primary metrics
available and explain that a new opt-in measurement is required. Opening such
a measurement while APFS DATA is selected preloads primary allocation and
then changes the selected button, with a visible explanation. Source backlinks
also preload before changing the accepted view: a known unavailable source
uses allocated directly; a source outside the recent history is tried with
APFS DATA and retried with allocated only for the explicit unavailable error.
Missing sources and stale reads preserve the accepted view and metric.

APFS baseline choices retain the existing exact root/policy/OS/volume/time gates
and require both measurements to be primary-complete, supported algorithm and
capture versions, and comparison-ready. The backend remains authoritative.
Fallback or unavailable comparison paths direct users to Allokeret/Logisk or a
new capture of the same root. Primary baseline behavior is unchanged.

Fallback explains that primary hardlink allocation was retained. File
nonowners show the saved global owner reference and a brief attribution reason;
a zero own charge does not prove empty or reclaimable content. Directory own
allocation of zero does not trigger this file explanation. No owner navigation
or broader private/unique/reclaimable claim is introduced.

## Verification

Use the pinned toolchain and environment in [the capture contract](apfs-clone-capture.md).
`scripts/build-local.sh` runs the workspace tests and builds the local CLI.
The opt-in native test is:

```sh
"$capture_toolchain/cargo" test -p macscan-analysis --test clone_capture \
  native_owned_apfs_clone_capture_roundtrip --locked --offline -- \
  --ignored --exact --nocapture
node --test ui/*.test.mjs
```

The actual owned APFS fixture uses native clonefile (never copy fallback), a
hardlink, independent resource fork, modified partial clone and punched sparse
file. Its file-only raw, hardlink-accounted and limited allocations are
5,312,512, 4,263,936 and 2,166,784 bytes respectively; directory allocation is
accounted separately. A later DATA mutation verifies clone-only compare/growth
while primary comparison and SQL changes remain empty. Fixtures are cleaned on
normal unwinding; no user/provider tree or desktop application is scanned or
launched. Synthetic tests cover inconsistent groups and aliases, partial same-ID
sharing, invalid/missing/zero/cross-device attributes, unknown filesystem,
partial/global-invalid subtree sums, owner outside scope, exact large integers,
version rejection and capture-off physical schema. Existing cancellation and
budget tests exercise the same finalization/publication path.


Desktop regression commands (synthetic command responses, no native scans):

```sh
export PLAYWRIGHT_MODULE=/opt/homebrew/lib/node_modules/@playwright/cli/node_modules/playwright/index.mjs
/opt/homebrew/bin/node ui/apfs-data-clone.browser.mjs
/opt/homebrew/bin/node ui/focused-rescan.browser.mjs
/opt/homebrew/bin/node ui/coverage.browser.mjs
/opt/homebrew/bin/node ui/growth.browser.mjs
/opt/homebrew/bin/node ui/file-profile.browser.mjs
```

The ignored `write_issue_8c_native_acceptance_fixture` integration test is a
separate explicit opt-in (`MACSCAN_ISSUE_8C_FIXTURE=create-owned-fixture`). It is
scoped to this checkout's `evidence/issue-8C/native-source` and `native-history`,
refuses either existing path, and leaves its actual source/history for native
acceptance. It creates `project/Cargo.toml` plus `target/{source,full,forked,
partial,hardlink,sparse}` with the actual operations above. Its map records
IDs, entry IDs, manifests, totals, provenance and cleanup. The latest captured
measurement is a rescan of the uncaptured root to exercise source fallback.
The opt-in generator never runs a machine plan or scans user/provider trees.

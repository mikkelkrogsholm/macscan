# CLI response contract 3.0

The CLI emits `schema_version: "3.0"` and an explicit command `outcome`:
`ok`, `partial`, `failed`, or `cancelled`. The snapshot manifest, Parquet schema,
native engine/desktop responses and existing snapshot readers remain version 2.0.
This is a CLI migration, not a rewrite of retained measurements.

## Exit codes and errors

| Exit | Outcome | Meaning |
| --- | --- | --- |
| 0 | ok | Requested operation completed with the coverage required below |
| 10 | partial | Usable result with incomplete requested measurement coverage |
| 130 | cancelled | This process observed cancellation |
| 2 | failed | Invalid arguments/input or an invalid query/response request |
| 3 | failed | I/O or execution failure, including broken stdout |
| 4 | failed | Requested root, store, scan or entry not found |
| 5 | failed | Storage, metadata or workspace budget exhausted |
| 6 | failed | Unsupported operation/metric or incompatible measurements |
| 7 | failed | History is locked by an active scan or cleanup |

Errors contain `error.code`, `operation`, `detail`, and `retryable`. Codes include
`INVALID_ARGUMENT`, `INVALID_INPUT`, `NOT_FOUND`, `UNSUPPORTED`,
`UNSUPPORTED_METRIC`, `UNSUPPORTED_WORKERS`, `ALREADY_EXISTS`, `BROKEN_PIPE`,
`METADATA_LIMIT`, `CANCELLED`, `HISTORY_BUSY`, `WRONG_ID_TYPE`, and `IO_ERROR`.
`HISTORY_BUSY` is retryable after the active operation finishes. Unknown I/O
errors are not automatically retryable. A hint is included only when a concrete
next action is supported; it does not promise that retry will repair an error.
`METADATA_LIMIT` covers the existing storage/workspace guard, including low free
space. No automatic retries or permission changes occur.

## Outcome by command

| Command | Coverage used for outcome |
| --- | --- |
| `scan ROOT` | The newly measured root's `selected_coverage.complete` |
| `scan --whole-mac` | All planned members: complete measured volumes required for 0; coverage gaps/unmounted volumes give 10; execution/storage errors give 3/5 and SIGINT gives 130 |
| `summary`, `children`, `largest`, `inspect` | The selected `--entry` subtree's `selected_coverage.complete`; default is root entry 0 |
| `profile`, `insights` | The selected saved subtree's `coverage.complete` |
| `growth` | Both corresponding saved subtrees' combined `coverage.complete` |
| `query` | Whole participating measurement(s), conservatively, even for SQL selecting one row, a constant or NULL |
| `compare` | Both participating whole measurements' combined `coverage.complete`; incompatible or unavailable metrics fail instead |
| `coverage` | Effective whole measurement status; delivering diagnostics does not establish full disk coverage |
| `scans`, `history-prune`, `volumes`, `capabilities`, `schema`, help, version | Successful execution; partial measurements listed in a catalog do not make catalog delivery partial |

An `ok` selected child can belong to a `partial` measurement. Existing `status`
fields keep their historical meaning: navigation preserves root status, while
some analysis commands already report selected-scope status. The additive
`measurement_status` records the effective root measurement status separately
from these selected-scope fields and `outcome`. Existing read-time volume
invalidation can make this effective status partial. Comparison also exposes
`before_status`/`after_status` where already supported; growth exposes its existing
`before_complete`/`after_complete` coverage evidence. NULL values mean unknown,
not zero, and do not themselves determine outcome. Pagination truncation is not
incomplete measurement coverage.

## Channels, framing and bounds

Successful and usable partial results go to stdout. Errors go to stderr, with
stdout empty when failure occurs before output. Optional progress also goes only
to stderr; NDJSON progress has `type: "progress"`. An output failure can leave
an incomplete stdout byte stream; consumers must check process exit and stderr.
No stack traces or file-by-file progress are emitted on stdout.

JSON has one result object. NDJSON has a `start` envelope, zero or more
`{"type":"record","record":...}` data rows, and an `end` envelope. Both envelopes
carry schema 3.0 and the same outcome. Rows preserve their existing data shape;
version/outcome belong to the envelopes. The end envelope includes measurement
identity and `retained` when applicable. A failed NDJSON command instead emits a
single stderr object with `type: "error"` and the same schema/outcome/error shape
as JSON. The requested format is recognized even for argument-validation errors.

Every final stdout response, including all NDJSON frames together, is at most
8192 bytes. A final error object is also at most 8192 serialized bytes, including
JSON escaping and newline. Diagnostic detail/warnings may be cut at UTF-8
boundaries, with explicit `detail_truncated`/`retention_warning_truncated` flags
on errors. Exact result values retain their existing representations (including
decimal strings for SQL integers). Large result pages use the existing cursor or
SQL offset; follow it unchanged. No number rounding is used to meet the budget.

## Publication, cancellation and retention

A successfully published scan has `scan_id` and `retained: true` with `--save`
or `--output`. A preview uses a private temporary store and reports
`retained: false`; that store is removed when the operation ends. If report
construction or stdout delivery fails after publication, the final error keeps
that new scan's identity and retained flag. A retained scan can therefore be
usable even when its command fails to deliver a report.

The existing three-run metadata retention policy runs after successful
publication, independently of report delivery. Cleanup failure is a
`retention_warning`, not a fictitious scan failure. Cancellation observed before
publication does not publish a new snapshot or evict prior history. Cancellation
observed immediately after publication keeps the published identity and skips
cleanup. Effects already completed before a later signal are not rolled back.
SIGINT is cooperative and yields exit 130 when observed; no promise is made to
interrupt a blocked filesystem syscall immediately.

## Migration from CLI 2.0

Consumers must switch on `schema_version` and `outcome`, then handle the explicit
exit matrix. CLI 2.0 used exit 5 for usable partial scans/profiles/insights/growth;
CLI 3.0 uses exit 10 for usable partial results consistently across commands.
Exit 5 now exclusively indicates a resource-budget failure. Root navigation,
SQL, compare and diagnostics now also communicate incomplete requested coverage
through exit 10. Busy now has a typed exit 7 and `retryable: true`. Existing
result fields remain available; store/native formats are unchanged.

Whole-Mac CLI scanning uses the same saved sessions and volume snapshots as the
desktop app. Existing data is not migrated. Its subprocess acceptance is covered
by `crates/cli/tests/whole_mac.rs`, alongside the standalone contract tests.

Verification is in `crates/cli/tests/contract.rs`, the existing CLI integration
suites, and the CLI's escaped-diagnostic budget unit test. They use synthetic
saved metadata and owned filesystem fixtures, not real scan history.

## Discovery (CLI-02)

`schema` remains the full machine view, including SQL columns and analysis
contracts. Its `commands` array lists installed command names. Use
`schema --command NAME` or `NAME --help` (also `NAME -h`) for the same short,
command-local description. Global `--help`, `-h`, or no arguments selects the
full schema. Help accepts only its help flag and optional `--format`;
`schema --command NAME` accepts optional `--format`. JSON is the default;
explicit `--format ndjson` preserves the CLI's start/record/end framing.
Both encodings, including all frames, fit 8192 bytes. An unknown command is a
CLI 3.0 `INVALID_ARGUMENT` error on stderr with exit 2 and empty stdout.

Discovery identifies the CLI package version, build revision, engine version,
retained data schema (2.0), and the legacy native engine schema separately.
The revision is embedded during compilation, with a `-dirty` suffix for tracked
working-tree changes; a source archive without Git reports `unknown`. Discovery
never runs Git or accesses the network at runtime. A copied release binary and
its bundled DuckDB library work from another directory without a `.git`
directory, GUI, existing store, or HOME. Help and discovery neither create a
store nor prune history. They do not open a measurement or enumerate scan roots.

`crates/cli/src/discovery.rs` maintains the command names and applicable option
descriptions. The runtime parser uses those descriptions to reject options
outside the selected command; parsing and value validation remain ordinary
Rust functions. The subprocess discovery suite checks advertised options
against accepted/rejected invocations, alongside the existing value-validation,
coverage, and pagination tests. Later commands must extend this same description
and their own tests when implemented. Whole-Mac scanning is still reserved for
CLI-04; run history is installed by CLI-03 below.

Short help includes required operands, snapshot/entry ID roles, defaults and
ranges, mutually exclusive filters, output channels, exit codes, and effects.
`--` ends option parsing, so `scan -- --help` scans a relative directory named
`--help`; a value token named `--help` is also a value, not a help request.
A normal scan uses a private temporary store, removed when the operation ends.
`--save` retains metadata; `--output PATH` aliases the store path and implies
saving. Store/output paths must be absolute without dot/parent components;
when repeated the last store/output path wins. A successful saved publication
can prune validated Macscan metadata to the latest three runs. Grouped whole-Mac
history counts as one run. This metadata retention is distinct from
`scanned_files_cleanup: false`: scanned source files are never deleted.
The default metadata allowance is 4 GiB for a new measurement; explicit
`--metadata-budget` limits old plus new retained metadata. The separate workspace
and free-space guards still apply; see [history-retention.md](history-retention.md).

Capabilities describe the installed metadata/Parquet backend, not the legacy
engine prototype's acceptance status. Logical and hardlink-accounted allocated
metrics are available; private bytes remain unsupported. Opt-in
`--capture-apfs-clones` supports the limited full-DATA correction within a
measurement root, with partial/non-DATA sharing retained. It does not establish
unique, private, or reclaimable bytes. No atomic filesystem snapshot is claimed.


## Run history and CLI 3.0 list migration (CLI-03)

`macscan scans` now lists global **runs**, newest first by exact numerical ID
components (timestamp, process and optional sequence), matching retention. CLI 2.0
listed individual snapshots. Migrate snapshot-list consumers to
`macscan scans --view snapshots`; this explicit view includes `scan_id`,
`snapshot_id`, root, saved status/time, entries, coverage and `metadata_bytes`.
All views use default 10/max 100 rows, the shared 8192-byte JSON/NDJSON budget,
and opaque `next_cursor` values. Pass each cursor unchanged to the same view and
run. Byte compaction resumes after the last delivered row, including volumes
without a snapshot. A single oversized row fails explicitly rather than being
silently skipped. If retention removes a cursor anchor, restart the listing.

Run rows distinguish `kind: standalone` and `kind: whole_mac`, with `run_id`,
`id_role: run`, saved status/time, member/snapshot counts and coverage. A
standalone run has one snapshot; its `run_id` and `snapshot_id` currently have the
same value but distinct roles. A whole-Mac run can have zero or many snapshots.
Its run ID is never a snapshot alias. The page describes the fixed three-run
retention policy; listing itself does not prune, reclaim stages, lock for writing
or create a store. A genuinely missing store is a successful empty list;
measurement queries against a missing store still fail with exit 4.

Resolve members with `macscan scans --run RUN_ID`. The page retains `run_kind`
and saved `run_status` even when it has no members; command `status: ok` and
`outcome: ok` describe successful catalog delivery. Each row includes stable
`member_index`, terminal `phase`, saved snapshot status and reason when known.
Saved members expose both `snapshot_id` and compatible `scan_id` for
`summary`, `children`, `inspect`, `insights`, `profile`, `coverage`, `compare`,
`growth` and `query`. Unmounted, error, cancelled and not-scanned volumes retain
their rows; absent snapshots, entry counts and coverage are NULL, not zero.
An empty cancelled or partial run remains valid. Legacy partial/cancelled
sessions may preserve pending or unknown phases without snapshot references;
these remain visible with NULL measurement data and block all pruning. A
complete empty run, corrupt session, dangling reference or inconsistent saved status fails
with a structured error and preserves history. Older compatible snapshots and
optional absent session fields require no migration.

Passing a whole-Mac run ID to a snapshot analysis command returns exit 2,
`WRONG_ID_TYPE`, with a hint to use `scans --run`. `--run` and
`--view snapshots` are mutually exclusive. OS capacity appears separately under
`os_capacity` (run) and `os_used_bytes` (member). No file totals are aggregated
across volumes and no unique or reclaimable capacity is inferred. App history
uses the same validated groups and keeps enriched child metadata for baselines,
source navigation and APFS analysis even when the sidebar hides those children.

The process fixtures in `crates/cli/tests/history.rs` cover mixed and shared
runs, exact numeric ordering, both encodings and byte-limited pagination,
terminal missing-volume states, empty/corrupt/dangling history, existing snapshot
analysis and four-to-three retention. They use owned synthetic files only.

## Whole-Mac orchestration (CLI-04)

```sh
macscan scan --whole-mac --save --progress ndjson
macscan scans --run RUN_ID
macscan summary --scan VOLUME_SCAN_ID
macscan coverage --scan VOLUME_SCAN_ID
```

`scan ROOT` remains available. ROOT and `--whole-mac` are mutually exclusive,
regardless of order. Whole-Mac also rejects `--metric` and `--limit`: its final
response is a run receipt, not a selected-file query. Select metrics and paginate
with the saved volume analysis commands. All option validation happens before
inventory, store creation or traversal. `--whole-mac` is a boolean flag.

Both clients call `macscan_analysis::machine` for APFS planning, sequential
streaming scans, session publication and loading. The plan includes mounted and
unmounted APFS volumes on the startup physical disk's containers, preserving the
app's existing ordering. External disks and disk images are excluded. System
firmlinks are excluded from `/`; Data is measured at its canonical root. No
provider policy, scanner or background service is added.

`--max-entries`, `--max-duration` and `--memory-budget` apply separately to each
volume. `--capture-apfs-clones` remains default-off and is passed to every
volume. One held store lock covers all children and final session publication.
The default 4 GiB new-run allowance is computed once and shared across the
children. Explicit `--metadata-budget` continues to cap combined old and new
metadata; existing workspace and free-space guards remain active. Session
receipts preserve the existing small terminal-diagnostic publication semantics
even after a child exhausts its measurement budget.

`--save` (or `--output DIR`) is explicit. Without it, a private temporary store
is removed at command end and the receipt says `retained:false`,
`temporary:true`; its run and child IDs cannot subsequently be queried.
Successful saved groups count once toward three-run retention. No pruning
happens between children. Execution failure or cancellation preserves earlier
history and completed published children. Unmounted volumes and partial root
coverage are observations, distinct from execution failures. Ordinary volume
I/O failures yield generic execution exit 3; detailed volume errors remain in
`scans --run`. Storage/workspace failure uses typed exit 5, never string matching.

Successful and usable partial receipts go to stdout. Failed/cancelled receipts
use the existing stderr error envelope with empty stdout. Every receipt remains
at most 8192 bytes in JSON and NDJSON, with explicit `run_id`, `kind`, `retained`,
`status`/`measurement_status`, `outcome`, coverage/member counts and a member-query
reference. On errors, `status:"failed"` describes the command;
`measurement_status` preserves the saved session's partial/cancelled/complete
state. Full membership and saved/unmeasured identities are paginated through
`scans --run`; no volume array appears in the final receipt. OS capacity stays
separate from file measurements and is never represented as reclaimable bytes.
Missing or overflowing OS capacity is `null`; capacity totals use checked exact
arithmetic. The app preserves unknown capacity in its overview and labels it
`Ukendt`, without deriving residual allocation or overlap from unknown inputs.

If final session publication fails after child publication, `session_published`
is false, while retained child counts and `recovery.first_scan_id` remain
visible. `recovery.command` identifies the snapshot catalog. The unpublished
run ID is a recovery identity, not a claim that `scans --run` can load a group.
No inconsistent group is written if a published child manifest cannot be read.
Cleanup warnings remain `retention_warning`, not failed scans. Broken stdout
preserves the same run identity and retained state in the stderr error.

Progress stays on stderr: compact parseable NDJSON with plan/phase,
`volume_index`, member and saved-member counts, entries and elapsed milliseconds.
Traversal events are capped at five per second, plus one plan event and one
terminal event per visited volume. Phase alternation cannot bypass that cap.
Terminal events identify the completed volume. There are no per-file lines or
invented completion percentages. Desktop progress retains its existing full
`machine` payload through its adapter.

The noninstalled Cargo `whole_mac` test host invokes the exact CLI library
entrypoint with an owned synthetic planner. Its parent launches actual
subprocesses for full, partial, unmounted, ordinary I/O, storage, busy, SIGINT
before any child and after child publication, temporary/capture/options,
publication recovery, broken stdout and bounded JSON/NDJSON behavior. The
installed binary binds only the real planner: there is no public test flag,
environment switch or arbitrary-plan input. Automated tests never call the real
machine planner or inspect user history. A real whole-Mac scan still requires
separate user authorization.

## Local installation and publication receipts

See [agent CLI installation and command flow](agent-cli.md) for the explicit
prefix installer, matching clean CLI/app build identities, bundled runtime,
update/uninstall behavior and owned-fixture acceptance. Both binaries expose
headless `--version` before HOME/history/UI initialization. Installed CLI discovery
requires no repository or GUI process.

Saved successful root/group publication adds `retention`, the existing pruning
planner's applied receipt. `kept_summary` lists up to three retained run IDs;
`removed_summary` lists up to eight removed run IDs, and
`removed_snapshot_summary` up to eight removed snapshot IDs. Counts are exact;
`summary_truncated` and `snapshot_summary_truncated` identify omitted IDs.
On an output error, diagnostic summaries may be shortened further to preserve
the 8 KiB bound (including `kept_summary_truncated`); counts remain exact. The
policy and on-disk formats are unchanged. Errors in pruning remain warnings and
do not turn an already published measurement into a scan failure.

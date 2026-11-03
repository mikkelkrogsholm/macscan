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
`METADATA_LIMIT`, `CANCELLED`, `HISTORY_BUSY`, and `IO_ERROR`.
`HISTORY_BUSY` is retryable after the active operation finishes. Unknown I/O
errors are not automatically retryable. A hint is included only when a concrete
next action is supported; it does not promise that retry will repair an error.
`METADATA_LIMIT` covers the existing storage/workspace guard, including low free
space. No automatic retries or permission changes occur.

## Outcome by command

| Command | Coverage used for outcome |
| --- | --- |
| `scan` | The newly measured root's `selected_coverage.complete` |
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

## Migration from CLI 2.0 and remaining acceptance

Consumers must switch on `schema_version` and `outcome`, then handle the explicit
exit matrix. CLI 2.0 used exit 5 for usable partial scans/profiles/insights/growth;
CLI 3.0 uses exit 10 for usable partial results consistently across commands.
Exit 5 now exclusively indicates a resource-budget failure. Root navigation,
SQL, compare and diagnostics now also communicate incomplete requested coverage
through exit 10. Busy now has a typed exit 7 and `retryable: true`. Existing
result fields remain available; store/native formats are unchanged.

Whole-Mac CLI commands are not introduced here. Issue #19's acceptance for
partially saved/cancelled whole-Mac CLI results remains open and must be verified
with issue #22. Existing desktop whole-Mac data is not migrated by this change.

Verification is in `crates/cli/tests/contract.rs`, the existing CLI integration
suites, and the CLI's escaped-diagnostic budget unit test. They use synthetic
saved metadata and owned filesystem fixtures, not real scan history.

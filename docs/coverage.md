# Saved measurement coverage diagnostics

`Store::coverage(scan_id, reason, cursor, limit, cancel)` and
`macscan coverage --scan ID [--reason REASON] [--cursor CURSOR] [--limit N]`
read immutable saved metadata. The CLI accepts normal `--store` and
`--format json|ndjson`. Default page size is 10; allowed sizes are 1–100.
There is no selected-entry argument: all counters describe the **whole
measurement**, even when examples are filtered or paged. They are events,
not unique areas, files, bytes or a coverage percentage. Several events may
refer to the same saved entry.

New measurements initialize `Scan.context.coverage_diagnostics` with version 1,
quality `event_counts`, unit `events_not_unique_areas`, counters and examples.
New saved manifests contain the additive `coverage_diagnostics` object;
measurement context excludes that duplicate object. Legacy engine save/load
restores it into the Scan context. Existing total `issue_count`/`error_count`
and `excluded_count` retain their meanings. Every occurrence increments exactly
one reason before example retention is considered.

| Reason | Meaning |
| --- | --- |
| `access_denied` | I/O PermissionDenied, including EACCES/EPERM |
| `provider_excluded` | Known provider path or dataless directory deliberately omitted |
| `volume_boundary` | Device boundary or explicitly annotated volume policy path |
| `firmlink_boundary` | Explicitly annotated startup-volume firmlink policy path |
| `scanner_output` | Scanner-owned metadata/output path with known provenance |
| `disappeared` | I/O NotFound while observing an entry |
| `explicit_limit` | Existing traversal limit or deliberate unannotated user exclusion |
| `io_error` | Other metadata, enumeration or directory-open I/O failure |
| `volume_changed` | Volume identity/availability changed during the measurement |

Raw codes distinguish e.g. `EXPLICIT_EXCLUDED`, `SCAN_LIMIT`, `DEPTH_LIMIT`,
`METADATA_ERROR`, `ENUMERATION_ERROR` and `OPEN_DIRECTORY_ERROR`. Error mapping
uses `io::ErrorKind` and raw errno, never localized error text. Operations are
saved separately. The desktop startup policy annotates existing exclusions;
it does not change their path set or policy fingerprint. Generic exclusions
are never guessed to be scanner output from their display names.

Each reason retains at most two examples, independent of the legacy first-128
issue list. Detail is bounded to 160 UTF-8 bytes with `detail_truncated`.
Legacy issue details are bounded to 80 bytes and their total sample metadata
has a 40 KiB budget. Actual names up to 255 bytes retain exact hex identity;
oversize synthetic child identities are omitted with an explicit truncation
flag. Counters continue after both retention caps. `samples_truncated` compares
retained examples with the known total; `truncated` instead means another page
of retained examples exists. No complete distribution is inferred from samples.
A manifest over the existing 64 KiB reader limit fails publication as a resource
error; previous snapshots remain intact.

## Report contract for native presentation

The report includes saved `started_at`/`ended_at`, `scan_id`, saved status,
`scope: whole_measurement`, quality, diagnostics version, reason filter,
whole-measurement `reason_counts`, `error_count` and `excluded_count`. Event
counts and known totals are exact decimal strings. All nine reason counters
remain present regardless of the example filter/page. A versioned measurement
with no events has nine zero counters. No coverage percentage is provided.

Records carry reason, raw code, operation, bounded detail, errno/I/O kind where
available, saved entry ID and an actionable Danish `next_step_da`. The saved
entry ID can be passed to existing `inspect --scan ID --entry ID`; this remains
usable when the displayed path is shortened. A failed child metadata lookup
retains the actual saved parent plus the raw child component, without inventing
a saved child ID. An enumeration failure has only directory context because
its failed child is unknown. `path_context` makes this distinction explicit.
`path_available: false` and null path fields mean no saved entry resolves the
sample; counters still remain useful. Invalid legacy child components are
omitted and marked `child_identity_invalid`.

Path display is bounded to 512 UTF-8 bytes; `path_truncated` is explicit and raw
path hex is null when shortened. The query bounds raw paths before crossing
the database result budget. Exact child identity is separate from display.
Never treat a truncated display as an executable path. Both JSON and NDJSON
are bounded to 8 KiB. The existing response compactor may return fewer records
than requested; `next_cursor`/`next_offset` follow the records actually returned.
Legacy in-memory `Scan::query` keeps the full counter distribution but omits
versioned examples with `samples_omitted_for_output_budget`; its existing raw
issue examples may also be trimmed with `issue_samples_truncated`.
Cursors bind scan, diagnostics version (including legacy), reason and offset.
Changing the reason or scan rejects the cursor. Native callers should use the
same Store report, not recount the examples or derive subtree counts.

Missing/unsupported/incomplete versioned counters produce
`quality: legacy_samples_only`, null diagnostics version and null reason
counts. Known saved error/exclusion totals and dates remain available. Saved
legacy examples have reason `unknown_legacy`; a reason filter yields no
classified examples. Opening old history does not rewrite it.

A globally changed volume invalidates every saved region and selection,
including previously completed children. Exact totals become null; observed
lower bounds remain. Coverage transitions cannot restore certainty under an
invalid volume, and comparison cannot claim removals or guaranteed decreases.
This differs from ordinary partial enumeration, where independently complete
children still retain useful coverage. Existing UUID compatibility checks
remain in force. On read, a proven mismatch between two saved fsids also
conservatively invalidates old root-only snapshots, without rewriting history;
missing legacy identity alone does not invalidate complete children. Failed/cancelled publication remains an execution failure,
not a fabricated completed measurement.

Provider/dataless/explicit root gates precede directory open and progress
branch discovery. Synthetic instrumentation verifies zero opens/enumeration
for excluded roots. This is a gate test, not native provider validation or a
no-hydration guarantee. No traversal limits, provider policy expansion,
security permissions, APFS accounting or cleanup operations are added.

## Evidence for native run B

The ignored `evidence/issue-6A/native-history` contains directly constructed
saved synthetic measurements, not scans of real paths. `fixtures.json` records
IDs, fixed dates, expected counters and sample truncation. Their roots begin
`/synthetic/`; these paths need not exist. Run B must still implement and
verify desktop presentation. Run A does not close the full issue.

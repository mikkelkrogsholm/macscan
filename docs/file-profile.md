# Saved file profiles (`file-profile-1`)

`Store::file_profile(scan_id, &FileProfileQuery, &AtomicBool)` and the CLI analyze
only immutable saved Parquet metadata. They never look up live paths or read
contents. The native UI can consume this shared API in a later delivery.

```sh
macscan profile --scan ID
macscan profile --scan ID --entry SAVED_ENTRY --metric logical --min-age-days 90 --min-bytes 1048576
macscan profile --scan ID --extension PDF --view types --sort count
macscan profile --scan ID --no-extension
macscan profile --scan ID --type-key ext:FF --view files --cursor CURSOR
```

Common options: `--store ABSOLUTE_HISTORY`, `--format json|ndjson`, `--limit 1..100`
(default 10), `--cursor`. Defaults: entry 0, allocated metric, files view, bytes
sort, no positive filters. `--sort count` requires types view. `--extension`
accepts a suffix without a dot; ASCII letters are case insensitive. Select one
of `--extension`, `--no-extension`, or `--type-key`. Filters combine with AND.

## API and exact values

`FileProfileQuery` fields are `entry: u32`, `metric: Metric`,
`min_age_days: Option<u64>`, `min_bytes: Option<u64>`, `type_key: Option<String>`,
`types: bool`, `sort_count: bool`, `limit: usize`, `cursor: Option<String>`.
`FileProfileQuery::extension_key("PDF")` converts a human suffix into its raw key.

The schema remains `2.0`; `command=profile`, `profile_version=file-profile-1`.
The envelope includes `scan_id`, `started_at`, `measured_at`, exact saved
`reference_unix_ns`, exact `age_threshold_unix_ns`, `filters`, `metric`, `mode`,
`sort`, `query_entry_id`, saved `scope`, selected `coverage`, `population`,
filtered `totals`, `records`, `offset`, `truncated`, `next_offset`, `next_cursor`.
Bytes, counts, timestamps and entry IDs are decimal strings or null. Use exact
integers such as BigInt, never JavaScript Number, for these values.

`scope` contains verified saved `entry_id`, `path_display`, `path_bytes_hex`,
relative raw hex `key` (null in legacy measurements), and `coverage_complete`.
Scope uses component boundaries in saved raw paths. Missing entries fail rather
than widening to the root. A regular-file scope includes that file alone.

`population` describes **all observed regular files in the selected scope**, before
filters: `file_path_count`, `unknown_mtime_count`, `future_mtime_count`,
`unassessed_mtime_count`, `unknown_type_count`, `unknown_size_count`. Future times
can only be assessed when the saved reference exists. `totals` describes the
**entire filtered observed population**, independently of pagination:
`bytes`, `observed_lower_bound_bytes`, `file_path_count`, `unknown_size_count`,
`coverage_complete`. It remains visible on every page.

File records contain `entry_id`, `drilldown_entry_id`, `drilldown_scan_id`,
`path_display`, exact `path_bytes_hex`, `type_key`, `type_display`, `bytes`,
`observed_lower_bound_bytes`, saved `mtime_unix_seconds`, nullable `future_mtime`,
and `coverage_complete`. Inspect the returned saved scan/entry pair using the
existing entry API. Type records contain `type_key`, `type_display` and the same
aggregate fields as `totals`. Select that exact `type_key` with `types=false`
to inspect contributing files. Display labels never determine identity.

## Age and type semantics

Age means **ikke ændret siden** the saved threshold; it does not establish that
a file is unused. For N days, the inclusive threshold is exactly
`saved ended_unix_ns - N * 86400 * 1000000000`. A file qualifies when its saved
mtime in integer seconds, converted exactly to nanoseconds, is <= that threshold.
Directory timestamps never affect file age; a new file in an old directory stays
new. Null or missing mtime is unknown. Saved zero means 1970 and negative values
mean before 1970. Future mtime is distinct. Unknown/future times are excluded
when an age filter is present. A missing or invalid saved reference produces
null reference/threshold without filtering; requesting an age filter fails
explicitly. The host clock is never a substitute.

The type hint uses only the saved raw basename's final dot and a nonempty suffix.
No dot, a dotfile with no later dot, and any trailing dot (including `a.b.`) mean
no extension (`type_key=none`). `.hidden.TXT` means `ext:747874`. ASCII suffix
letters fold to lowercase bytes; other bytes are preserved. `ext:HEX` uses
uppercase hex digits; two invalid-UTF8 suffixes remain distinct even if their
lossy `type_display` labels coincide. No MIME sniffing occurs. Legacy missing
raw basenames and a file-root's empty basename use the saved raw path basename;
missing timestamps remain unknown. Missing raw scope data fails explicitly.

## Accounting, coverage and bounds

Only regular-file rows contribute: no directories, symlinks, special entries or
subtree totals. Logical bytes count each saved file path. Allocated bytes retain
the saved hardlink owner even outside the selected scope. File counts include
hardlinked paths and zero-byte paths; types sorted by count expose many small
files without increasing the bounded response. Byte sums use DuckDB HUGEINT,
including values beyond u64. They do not establish unique APFS or reclaimable
space, process attribution or deletability.

Without a size filter, unknown-size files remain counted and observed lower
bounds remain visible. With `--min-bytes`, the exact selected metric must be known
and meet the inclusive threshold; unknown sizes are excluded. Population counts
still report them. Complete totals also require known sizes and complete saved
file coverage. In incomplete selected scope, `bytes=null`, observed sums and
counts are lower bounds for the filtered population. Unobserved files cannot be
counted or assigned to a type. A complete selected child of a partial root can
still return exact totals. Exit 5 means incomplete selected coverage.

Files sort by observed bytes descending, then exact raw path and entry ID. Types
sort by observed bytes descending then raw type key; count sort uses count
descending, then observed bytes descending, then type key. Cursors bind scan,
version, scope, metric, age/size/type filters, view and sort. Pass them unchanged;
page limit may change. Offsets advance by rows actually encoded. Both JSON and
NDJSON fit 8192 bytes, so a page may contain fewer rows than requested.

The existing 256 MiB DuckDB buffer budget, two threads, no disk spill and query
cancellation apply. There is no Rust copy of the saved file tree. Resource
rejections explain how to retry with `--entry ID` for a smaller saved directory.
This buffer bound is not a whole-process RSS guarantee. Snapshots are unchanged.

Synthetic-only validation lives in `crates/analysis/tests/file_profile.rs`, the
module's legacy unit regression, and `crates/cli/tests/file_profile.rs`. Set
`MACSCAN_PROFILE_EVIDENCE` to an absolute new history directory when running only
`retain_native_synthetic_history_when_requested` to retain the small synthetic
native handoff. It constructs Scan rows directly without scanning any paths.

# Saved growth explanations (`growth-1`)

`Store::growth(before_id, after_id, &GrowthQuery, &AtomicBool)` explains changes
between immutable saved measurements. No live lookup, content read, process
attribution, rename inference or cleanup action occurs. This API and CLI are the
shared contract for the later native app integration.

```sh
macscan growth BEFORE AFTER --metric allocated
macscan growth BEFORE AFTER --entry AFTER_PROJECT_ENTRY --grouping category
macscan growth BEFORE AFTER --entry AFTER_PROJECT_ENTRY --grouping area --view files --group GROUP_KEY
macscan growth BEFORE AFTER --view files --change removed --limit 10 --cursor CURSOR
```

Common flags: `--store ABSOLUTE_HISTORY`, `--format json|ndjson`,
`--metric logical|allocated`, `--entry AFTER_ENTRY_ID`, `--limit 1..100` and
`--cursor CURSOR`. Defaults: allocated, entry 0, limit 10, grouping area, view
groups, change all, no group or cursor. `--grouping area|category` controls
attribution. `--view groups|files` chooses rollups or changed-file drilldown;
`--group KEY` selects the exact returned `group_key`, and
`--change all|grown|shrunk|added|removed|unknown` filters either view. Filters
can be combined. No SQL is required. A valid but absent group returns no rows;
incomplete selected coverage still returns null aggregate growth, never zero.

Rust fields match these flags: `GrowthQuery { entry, metric, grouping:
GrowthGrouping::Area | GrowthGrouping::Category, files: bool, group:
Option<String>, change: String, limit, cursor: Option<String> }`.

## Accounting and attribution

Contributions contain only changed regular-file paths. Directories, symlinks and
special entries contribute zero; directory subtree totals are never added to
child contributions. File/directory transitions count only the regular-file
side. Logical bytes count each regular-file path. Allocated bytes use each
measurement's saved `accounted_allocated_bytes`, including its one hardlink
owner; a subtree does not acquire another owner's allocation. Negative values
and sums beyond u64 use exact HUGEINT subtraction/aggregation. All byte values,
counts and saved IDs are decimal strings, or null for unknown values. Consumers
must use exact integer handling, not JavaScript Number for bytes.

Area keys are uppercase hex of saved relative raw `path_key` bytes up through
the first child of the selected scope (NUL-delimited path components). A selected
file groups under its own key. They use byte boundaries, including non-UTF-8
names; display paths never determine grouping. A project is an explicit saved
AFTER entry scope; this API adds no project-discovery engine.

Category keys are the existing insights category IDs. Both consumers use the
same `insights-1` catalog, contextual marker rules, nested precedence and ASOF
leaf assignment. A retained/new file uses its AFTER category; a removed file
uses its BEFORE category. A context-only reclassification with no metric change
produces no contribution. Moves are a removed path plus an added path. Different
area/category grouping can therefore net moves differently, without duplicating
any file's contribution. New/removed zero-byte paths remain counted even though
their byte contribution is zero.

## Response for native consumers

The envelope uses schema `2.0`, `command=growth`, `growth_version=growth-1`,
`rule_version=insights-1`, `before_scan_id`, `scan_id` (AFTER), `metric`,
`query_entry_id`, `grouping`, `mode=groups|files`, `group`, `change`, `status`,
`coverage`, `scope`, `interval`, `totals`, `contributions`, `records`,
`offset`, `truncated`, `next_offset` and `next_cursor`.

`scope.after` includes the verified AFTER `entry_id`, `path_display`, exact
`path_bytes_hex`, relative hex `key`, and `coverage_complete`. `scope.before`
selects the equivalent saved raw path key, never the same numeric entry ID;
it includes its saved entry ID/raw path and coverage. A provably absent BEFORE
region has a null anchor, not an invented root. Missing AFTER entries fail.
Legacy measurements without saved raw/context metadata fail explicitly as
unsupported; they are never silently widened to the whole measurement.

`interval` carries both saved start/end display dates, both exact saved
`*_ended_unix_ns`, and their exact difference `elapsed_ns`. If saved end-time
metadata is unavailable, the corresponding values/difference are null. No live
clock substitutes for a baseline. Compatibility is the existing comparison
contract: same saved root, policy, OS and volume identity, strictly chronological
BEFORE/AFTER starts.

`totals.before` and `totals.after` are **all regular files in the selected scope**,
independent of group/change filters: `bytes`, `observed_lower_bound_bytes`,
`file_path_count`. These are distinct from changed-file contributions.

Group records contain `group_key`, `delta_bytes`,
`changed_file_before_bytes`, `changed_file_after_bytes`,
`observed_changed_file_before_bytes`, `observed_changed_file_after_bytes`,
`file_path_count`, `new_count`, `removed_count`, `changed_count`, `unknown_count`,
`context_coverage=complete|partial`.
The before/after group values describe contributing changed files, not full area
or category totals. Area rows also contain display/raw paths and nullable saved
`before_entry_id`/`after_entry_id`; category rows contain `category`.
`contributions` has the same aggregate/count fields for the filtered selection.
`changed_count` counts grown/shrunk paths; new/removed/unknown are separate.
Counts describe observed saved contributing paths, including hardlinked paths;
hidden unobserved paths cannot be counted from metadata.

File rows contain `group_key`, `path_display`, relative exact `path_key_hex`,
nullable `before_entry_id`/`after_entry_id`, `before_bytes`, `after_bytes`,
`delta_bytes`, `observed_before_bytes`, `observed_after_bytes`, `path_bytes_hex`,
`context_coverage`, `change`, `drilldown_scan_id`, `drilldown_entry_id`.
For existing AFTER anchors, open that saved scan/entry. For removed paths with
no AFTER anchor, open the returned BEFORE scan/entry. Never apply a BEFORE entry
ID to AFTER. Group selection uses `--group` with the same grouping/scope and
`--view files`; inspection uses the existing saved-entry API. Explanatory
`accounting_note` and `attribution_note` are Danish and reusable by the app.

## Coverage, pagination and budgets

Coverage reuses the comparison's saved interval change points. Missing paths in
unreadable/excluded regions remain unknown, not false removals or safe negative
growth. Complete saved children inside partial measurements remain useful when
both selected regions are proven complete. File rows preserve independently
known sides/deltas even in partial scopes. Group/filtered aggregates conservatively
return null exact before/after/delta when selected coverage is incomplete or any
contributor side is unknown; observed before/after sums remain visible. This also
applies to empty filtered results. Unknown observations do not become zeros.
Coverage status, exact all-file totals and counts remain visible on every page.
`coverage.before_complete`, `after_complete`, `complete` refer to selected scopes;
exit code 5 indicates incomplete selected coverage.

Order: unknown deltas first, then descending absolute net delta, then stable
group key or saved raw file path key. Both JSON and NDJSON fit 8192 bytes. Pages
can contain fewer rows than the requested limit due to the output budget. Pass
`next_cursor` unchanged with the same before/after, metric, entry, grouping,
view, group and change filter. It also binds growth/catalog versions. The cursor
advances by rows actually encoded; page size may change. Mismatches fail.

The API uses the existing two-scan DuckDB 512 MiB buffer budget, two threads,
no disk spill and existing cancellation. Classification uses the shared candidate
interval events and lazy leaf views, not a Rust tree or all-entry ancestor
cross-product. Resource errors include the concrete recovery `--entry ID` for a
smaller saved directory; failed/partial top results are never presented as a
complete ranking. These limits are not whole-process RSS guarantees. No age,
diagnostics, APFS uniqueness or forecasting is inferred.

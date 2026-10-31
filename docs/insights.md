# Saved metadata insights (`insights-1`)

`Store::insights(scan_id, &InsightsQuery, &AtomicBool)` is the shared analysis API.
It reads only immutable saved metadata. It does not scan live paths, read file
contents, execute guidance, delete, download, change permissions or schedule work.
The native app consumes this API through a thin asynchronous Tauri command.

## Native app

Open a saved measurement and select **Indsigter** beside Indhold and Ændringer.
Choose **Hele målingen** or **Aktuel mappe / projekt**; graph and breadcrumb
navigation supplies the saved directory scope. The selected scope, saved end
time, metric, coverage and all-file totals remain visible. Category buttons
open contributing paths; **Alle kategorier** returns to rollups. **Næste side**
replaces the current page, using the actual opaque cursor and keeping the DOM
bounded to the returned page (default ten records, sometimes fewer). Metric,
category and scope changes discard cursors; obsolete responses are ignored.

A contributing path opens the existing file analysis at `drilldown_entry_id`,
where explicit Finder and copy actions remain available. Contributions exclude
separately classified nested areas, so raw directory totals can differ. Evidence
paths may be ancestors of the chosen scope; drilldown stays within that scope.
All UI explanations and API finding guidance are Danish. No guidance executes.
Unknown legacy context has an explicit status. The whole-Mac overview disables
Indsigter and directs the user to choose a scanned volume; OS totals are never
combined into category rollups.

```sh
macscan insights --scan ID --metric allocated
macscan insights --scan ID --category build_output --limit 10
macscan insights --scan ID --entry PROJECT_ENTRY_ID --metric logical
macscan insights --scan ID --category dependencies --entry SUBTREE_ID --cursor CURSOR
```

Use the existing `--store`, `--format json|ndjson`, `--limit 1..100` (default 10)
and `--cursor` options. Both encodings fit 8192 bytes. Invalid categories, missing
entries and mismatched cursors fail explicitly. SIGINT uses the existing query
cancellation path. Queries use the existing 256 MiB DuckDB buffer budget, two
threads and no disk spill; oversized analysis can fail rather than exceed that
budget. This is not a whole-process RSS guarantee.

## Consumer contract

The normal schema remains `2.0`; `insights_version` and row `rule_version` are
`insights-1`, independent of navigation's unchanged `context-1` rules. Defaults
are `entry=0`, `metric=allocated`, `category=None`, `limit=10`, `cursor=None`.
`mode=categories` returns mutually exclusive category rollups. Selecting a
`category` returns `mode=candidates`: paginated contributing directory scopes,
not a second directory-size accounting. Categories with no observed regular
files are absent; absence in an incomplete measurement is not proof of zero.

Each finding includes `category`, exact decimal-string `bytes` (or null),
`observed_lower_bound_bytes`, `file_path_count`,
`coverage_complete`, `context_coverage`, `rule_id`, `rule_version`, `explanation`,
`guidance` and `evidence`. Rollup `rule_id=multiple` means several rules
contribute; select that category for concrete evidence. Package-cache rollups
refer to candidate details for tool-specific guidance.

Candidate rows also include saved `entry_id`/`candidate_entry_id`, `path_display`,
exact `path_bytes_hex` and `drilldown_entry_id`. `entry_id` identifies the original
evidence scope. **Use `drilldown_entry_id` to navigate**: if a selected subtree is
inside an inherited candidate (such as a package under `node_modules`), that
anchor is clipped to the query entry. Bytes always describe only contributions
inside `query_entry_id`; the evidence path can be an ancestor outside it. Candidate
scopes can overlap as paths, but their assigned file contributions do not.

The envelope contains `scan_id`, `started_at`, `measured_at` (saved end time),
`metric`, selected `scope`, `query_entry_id`, `coverage`, `totals`, `category`,
`context_coverage`, `count_note`, `attribution_note` and existing warnings.
`totals` always covers all regular files in the selected scope, even when the
records are filtered to one category. Measurement coverage and contextual
classification confidence are separate. Missing legacy context produces
`context_unavailable`, never promotion of legacy `saved_category` to proof.

`bytes` is null for every category/candidate when selected-scope coverage is
incomplete, with observed lower bounds retained. This conservative choice does
not guess which category unobserved files belong to. A complete child scope can
have known totals inside a partial scan. Exit code 5 signals incomplete selected
coverage; existing failed-query/unsupported/cancel codes are reused.

Logical bytes sum regular-file lengths, including each hardlinked path.
Allocated bytes sum saved `accounted_allocated_bytes`, retaining the snapshot's
one hardlink owner even when that owner is outside a selected subtree or category.
Directories, symlinks and special entries contribute no bytes. `file_path_count` counts regular-file paths including hardlinks; it does not
count physical identities. Allocated bytes do not establish unique APFS or
reclaimable space. Metadata cannot prove process attribution or deletability.

Ordering is observed bytes descending, then category; candidates use observed
bytes descending, saved entry ID, rule ID. The opaque cursor binds scan, metric,
selected entry, category, query mode and rule version. Pass it unchanged with the
same query. Offsets advance by the records actually encoded, including when the
output budget removes records. Page size may change between requests.

## Fixed initial catalog

Rules use exact saved raw directory bytes/types, plus saved regular sibling
markers for contextual Rust/Node rules. A filename extension alone is not enough.
Conventional suffixes below are matched at slash boundaries; redirected custom
cache/model locations are not inferred from configuration or environment.

| Category | Saved directory suffix or evidence | Role and review guidance |
| --- | --- | --- |
| `build_output` | `target` beside regular `Cargo.toml` | Probable Rust output; review with Cargo |
| `dependencies` | `node_modules` beside regular `package.json` | Probable dependencies; review with project package manager |
| `build_hint` | `build`, `.next`, `DerivedData` without specific context | Unverified name hint; review purpose manually |
| `dependency_hint` | `.venv` | Unverified name hint; review purpose manually |
| `cache_hint` | `.cache`, `Caches`, `__pycache__` | Unverified name hint; review purpose manually |
| `xcode_derived_data` | `/Library/Developer/Xcode/DerivedData` | Build data; review in Xcode settings |
| `xcode_archives` | `/Library/Developer/Xcode/Archives` | Potentially valuable releases; review in Organizer |
| `simulator_devices` | `/Library/Developer/CoreSimulator/Devices` | Device and app state; review in Devices and Simulators |
| `docker_disk` | `/Library/Containers/com.docker.docker/Data/vms` | VM/container disk area; review in Docker Desktop |
| `orbstack_disk` | `/Library/Group Containers/HUAQ24HBR6.dev.orbstack/data` | VM/container data; review in OrbStack |
| `ollama_models` | `/.ollama/models` | Retained models; review in Ollama |
| `huggingface_models` | `/.cache/huggingface/hub` | Model hub; review with Hugging Face cache manager |
| `lmstudio_models` | `/.lmstudio/models`, `/.cache/lm-studio/models` | Retained models; review in LM Studio |
| `package_cache` | `/.npm/_cacache`, `/.cargo/registry/cache`, `/.cargo/git/db`, `/Library/Caches/pip`, `/.cache/pip`, `/Library/pnpm/store`, `/Library/Caches/Homebrew`, `/.bun/install/cache`, `/.cache/uv` | Review with the specific package manager named in candidate guidance |
| `unclassified` | No supported context, unverified target/dependency name, or project boundary | Personal data, backups and unknown data are not junk by default |

Specific catalog contexts outrank generic name hints and project boundaries;
within a priority the deepest candidate wins. Other rules retain context-1's
project-boundary behavior, including Node packages inside dependency areas.
Overlapping scopes cannot count the same file twice. The implementation creates
candidate scopes and interval-boundary events in DuckDB, then assigns leaves via
an ASOF join over a lazy leaf view that is aggregated without storing all leaves.
It makes no Rust copy of the tree and no file/ancestor cross-product.
Snapshots and manifests are never rewritten by insights.

Package-cache defaults verified against official documentation on 2026-10-31:
[Homebrew](https://docs.brew.sh/Manpage),
[Bun](https://bun.sh/docs/pm/global-cache),
[uv](https://docs.astral.sh/uv/reference/storage/).
Their location overrides are deliberately not read or guessed.

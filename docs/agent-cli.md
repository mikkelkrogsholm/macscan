# Local Macscan CLI for agents

Build and install are separate, explicit operations. On arm64 macOS 14+, the
project's locked/offline scripts produce ad-hoc signed local artifacts:

```sh
scripts/build-local.sh
scripts/build-desktop.sh
python3 scripts/install-cli.py install --prefix /chosen/prefix \
  --artifacts /absolute/release/dist --app /absolute/release/dist/Macscan.app
```

The installer requires Python 3.9+ and Apple's installed command-line tools
(`codesign`, `otool`). The installed command needs neither Python nor a checkout,
build tools, the GUI, or a symlink into `dist`. Choose the prefix explicitly;
`/chosen/prefix/bin/macscan` is a regular shell launcher and its payload is copied
to `/chosen/prefix/lib/macscan`. Prefixes containing spaces are supported. Existing
unrelated prefix files are preserved. Managed paths and ancestors must not be
symlinks. Unknown files or modified owned files cause a refusal, not replacement.

The installer prints the exact command path and both CLI/app identities. If the
bin directory is absent from PATH it prints the exact `export PATH=...` command;
it never edits shell profiles. Run that command in the desired shell, then use
`macscan` from any working directory. No global install is implied by a build.

Use `status --prefix /chosen/prefix` to read the installed identity. Update by
running the same explicit `install` command with newly built artifacts. CLI and
app must have matching clean Git build revisions, package/engine/data versions,
and bundled DuckDB hashes. Only the explicitly supplied new app artifact is
queried with its headless `--version`; no installed GUI is launched. Both
packaged binaries have only their bundle-relative runtime search path. Debug/test
binaries retain their local build fallback.

Every stage validates signatures, runtime paths and executable identity before
activation. Updates atomically replace the internal current-release link after
validation; a failed stage keeps the previous payload active. A prefix directory
lock prevents simultaneous installers. Cleanup after activation is explicitly a
warning on failure; recorded prior payloads can be validated and cleaned by the
next update, including partially completed cleanup. This is a local
ad-hoc release path, without notarization or an updater. Missing runtime files
produce an actionable launcher error and never fall back into `.build`.

```sh
python3 scripts/install-cli.py status --prefix /chosen/prefix
python3 scripts/install-cli.py uninstall --prefix /chosen/prefix
```

Uninstall removes only the validated owned launcher/payload. Installation, update,
status and uninstall do not create, copy, inspect or delete measurement history.
The CLI and app share the default `$HOME/Library/Application Support/Macscan` and
accept the same explicit `--store` directory. Pass the chosen store on every
analysis command when using a nondefault location.

## Short agent flow

Discover the installed contract before choosing a scan. The paths below are
placeholders for an explicitly chosen root and metadata destination:

```sh
macscan --version
macscan schema --command scan
macscan scan --help
macscan scans --store /chosen/history
macscan scan /chosen/owned-files --save --store /chosen/history --progress ndjson
macscan scans --store /chosen/history
macscan scans --run RUN_ID --store /chosen/history
macscan summary --scan SNAPSHOT_ID --store /chosen/history
macscan insights --scan SNAPSHOT_ID --store /chosen/history
macscan coverage --scan SNAPSHOT_ID --store /chosen/history
# After a separately requested second measurement of the same root/policy:
macscan growth BEFORE_SNAPSHOT_ID AFTER_SNAPSHOT_ID --store /chosen/history
```

Use returned IDs literally. `scans` defaults to runs, including ordinary root
measurements; `scans --run RUN_ID` returns their saved snapshot members. Analysis
uses a snapshot ID, never a grouped run ID. `scans --view snapshots` explicitly
lists snapshots. Follow returned cursors unchanged; schema/help describe each
command's options. `query` is restricted to saved read-only SQL views.

A root scan without `--save` uses a private temporary metadata store removed at
command end. `--save` publishes immutable metadata and may prune validated old
Macscan metadata to the newest three runs. A whole-Mac group counts once. The
publication response's `retention` reports kept/removed run counts and bounded
`removed_summary` run IDs plus `removed_snapshot_summary` snapshot IDs, with
truncation flags. These snapshots then return exit 4. Cleanup errors remain
`retention_warning` alongside a saved result. Scanned source files are never
removed by this policy. Read-only commands do not scan or prune.

For a separately authorized whole-Mac measurement, the same run/member flow is:

```sh
macscan scan --whole-mac --save --store /chosen/history --progress ndjson
macscan scans --run RUN_ID --store /chosen/history
macscan coverage --scan VOLUME_SNAPSHOT_ID --store /chosen/history
```

This inventories startup physical APFS volumes using the app's shared planner;
external volumes/images are excluded. It does not grant permissions. It is not
part of automated acceptance on a real machine.

Check process exit and `outcome`: 0 complete, 10 usable partial, 130 interrupted,
2 input/wrong ID type, 3 I/O/runtime, 4 missing/pruned ID, 5 budget, 6 unsupported, 7 busy.
Partial is no longer exit 5. Inspect coverage before drawing conclusions. On
SIGINT, wait for this process to terminate; a tool timeout is not cancellation.
Do not automatically restart scans. Busy can be retried after the active job
finishes; inspect the error's `retryable` field. Missing runtime or version drift
requires installation repair, not a new scan. See [CLI contract](cli-contract.md)
for framing, exit migration and typed errors.

JSON/NDJSON final responses are bounded to 8192 bytes, with progress only on
stderr. Exact numbers/IDs are preserved; SQL integers are decimal strings and
null means unknown. Metadata scans read no file contents. Allocated file bytes
are scoped OS accounting, not unique/private/reclaimable APFS capacity. The
measurement is not an atomic filesystem snapshot. Coverage gaps and contextual
category hints are not deletion approval.

## Reproducible owned-fixture acceptance

After building both artifacts from the same clean commit, build the existing
noninstalled whole-Mac test host in release mode using the same locked/offline
toolchain and `.build` target directory, then run:

```sh
python3 scripts/test-installed-cli.py --artifacts /absolute/release/dist \
  --whole-mac-host /absolute/release/.build/release/deps/whole_mac-HASH
```

The script creates an owned temporary prefix (with spaces), fixtures, history and
unrelated working directory. It invokes `macscan` through ordinary PATH. It checks
root and grouped discovery/member/analysis flows, compatible growth, fourth-run
retention, bounded JSON/NDJSON, progress and real process exit codes including
SIGINT; loader paths, missing dylib, version drift, failed staging, foreign-file
refusal, updates and uninstall. Temporary data and child processes are cleaned.
The existing noninstalled test host is the sole producer of synthetic grouped
measurements, invoking the exact CLI library entrypoint with its owned plan.
The actual installed release performs all subsequent grouped queries. The
production binary has no synthetic planner flag/environment bypass. This proves
shared saved-data compatibility, not real machine inventory or TCC access.

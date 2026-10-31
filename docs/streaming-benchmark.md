# Production streaming benchmark

`crates/analysis/examples/streaming_benchmark.rs` measures the current
`walk_with_progress_guarded` → `Store.scan_to_store` production implementation,
including batch Parquet writes, DuckDB hardlink charging, subtree aggregation,
final Parquet, manifest synchronization, cleanup, watcher join and publication.
The older engine `benchmark` and analysis `parquet_benchmark` examples use the
in-memory scan/save path and are not evidence for streaming performance.

Run explicitly on macOS with the pinned toolchain and existing local DuckDB:

```sh
export RUSTC="$HOME/.rustup/toolchains/1.99.1-aarch64-apple-darwin/bin/rustc"
export RUSTDOC="$HOME/.rustup/toolchains/1.99.1-aarch64-apple-darwin/bin/rustdoc"
export CARGO_TARGET_DIR="$PWD/.build"
export CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0 MACOSX_DEPLOYMENT_TARGET=14.0
"$HOME/.rustup/toolchains/1.99.1-aarch64-apple-darwin/bin/cargo" test \
  -p macscan-analysis --example streaming_benchmark --locked --offline
"$HOME/.rustup/toolchains/1.99.1-aarch64-apple-darwin/bin/cargo" run \
  -p macscan-analysis --example streaming_benchmark --release --locked --offline \
  -- --run-small > streaming-report.json
```

The example rejects a missing opt-in argument and debug performance runs. The
small fixture creates 1,920 regular files containing 1–257 bytes, 32 additional
hardlink names, 32 broad directories and a 24-directory nested chain, for 2,009
entries including the root. It owns a unique temporary parent containing both
target and store; it never scans an existing user tree. A drop guard removes
only this parent after success or recoverable failure. SIGINT/SIGTERM request
cancellation through the existing atomic cancellation path; SIGKILL/process
crashes cannot guarantee fixture cleanup. Normal execution reports cleanup on
stderr. No application launch, cache purge or global installation is involved.

Creation is followed by one warmup in each mode, then three measured runs per
mode. Modes are ordinary bulk operation and forced existing stat fallback.
The latter injects `ENOTSUP` before each directory's first bulk syscall; the
existing rewind/readdir/fstatat branch handles it. This selection exists only
in the developer profiling API, outside `ScanOptions` and policy fingerprints.
The report verifies that baseline uses actual bulk syscalls without forced
fallbacks, and that fallback performs no bulk syscall and does execute
fstatat/readdir. No traversal or finalization implementation is copied.

Large fixtures require a separate explicit `--allow-large 100000` or
`--allow-large 1000000` argument. These are **not** part of the standard run or
normal tests; do not run them without a separately authorized resource budget.
This benchmark does not decide a product performance threshold or justify
parallel traversal by itself.

## Timing definitions

All times are elapsed wall times with opt-in `Instant` observations. Ordinary
unprofiled syscall calls do not create per-call timers or print diagnostics.
Profiles are returned to the developer caller, never stored in manifests or
shown in application progress. Instrumentation preserves syscall errno,
including zero errno on readdir EOF.

- `total_ms`: pipeline call, from entry through publication and marker removal.
- `walk_inclusive_ms`: complete shared engine walk, including synchronous
  batch flush callbacks, root/context setup and volume observations.
- `flush_inside_walk_ms`: flush calls performed by the emit callback.
- `walk_excluding_flush_ms`: walk inclusive minus those flushes.
- `flush_tail_ms`: remaining batch write after the walk returns.
- `finalize_ms`: shared DuckDB finalization, including hardlink lookup,
  aggregation, coverage Parquet, final entry Parquet and totals query.
- `watcher_join_ms`: elapsed after pipeline body returns until scoped watcher
  termination. The existing watcher sleeps up to 200 ms between checks; its
  join time is included in the pipeline wall, not traversal syscall time.
- `publish_ms`: destination construction, atomic rename and marker removal.
- `other_ms`: total residual after walk inclusive, tail flush, finalize,
  watcher join and publish. This includes store locking/recovery checks, DB
  setup, manifest construction/sync, DB shutdown and raw/work cleanup.

The exclusive top-level sum is `walk_excluding_flush + flush_inside_walk +
flush_tail + finalize + watcher_join + publish + other`. `accounted_ms` checks
this sum against `total_ms`; inclusive walk is not added again.

Nested syscall measurements record counts and elapsed time for actual
`getattrlistbulk` (enumeration **and** metadata together), `readdir`, and
`fstatat` calls. Bulk records served from a decoded batch are not counted as
syscalls. These measurements also include the root branch-discovery pass.
They exclude root `symlink_metadata`, canonicalization, directory open/fstat,
volume/Foundation context helpers, record parsing, path construction,
progress callbacks, and other runtime work. Those costs, plus timing overhead,
remain in `walk_uninstrumented_residual_ms`. This residual does not establish
that all its time is CPU time. There is no fictional separate bulk metadata
versus enumeration percentage.

The benchmark uses `ScanOptions::full()`, matching application streaming calls;
existing history and workspace budgets still apply (including large opt-in runs).
The standard small fixture normally fits one tail batch at default settings.
The focused example test uses the existing memory budget option (256 KiB,
64 KiB minimum batch budget) with a smaller fixture to force in-walk and tail
flushes, and verifies exclusive accounting and saved parity. Product defaults
and resource policies are unchanged.

## Parity and resource interpretation

After an actual final Parquet roundtrip, every repeat and fallback result is
compared with the unprofiled warmup. A bounded aggregate SQL query through
`Store.query_sql(after, Some(before), ...)` uses a full outer join on raw path
bytes. It compares all saved non-ID columns, and resolves parent and hardlink
owner IDs to raw path bytes before comparison. It checks logical, raw and
hardlink-accounted bytes, observed lower bounds, coverage/enumeration flags,
kinds and identities. Manifest totals, entry/error/exclusion counts, diagnostic
counts and status must match; complete coverage and expected entry count are
required. Traversal IDs may differ. Query attachment also enforces production
compatibility checks. Validation is separately timed, outside scan wall.

`entries_per_second` uses complete pipeline wall including watcher wait.
Darwin `getrusage(RUSAGE_SELF).ru_maxrss` is reported in bytes before/after each
run. It is the **lifetime process high-water**, including fixture creation,
warmups, prior runs and validations; these runs are not fresh-child RSS peaks
and subtracting them does not give per-run memory consumption.

A benchmark-only observer samples the current process's descriptor count using
`proc_pidinfo(PROC_PIDLISTFDS)` and logical file lengths under owned `*.partial`
stages, plus the `work` subset (DuckDB database/spill). It sleeps 25 ms between
samples, so actual intervals are 25 ms plus observation work. Sampling
observation time and failures (including cleanup/publication races) are
reported. The descriptor buffer holds 8,192 records; overflow is a failed
sample, not a silently truncated peak. These are **sampled maxima**, not exact
peaks or physical allocated-byte measurements. Stage includes work; do not
sum them. Sampling itself opens directory handles, contributes to descriptor
counts and process resource use, and can perturb timing. The observer is
joined before validation; its join delay is outside the profiled pipeline.

The JSON report includes hardware, OS/kernel, filesystem, Rust version, DuckDB
version and deployment target. All runs share process and post-creation OS
caches; there is no cold-cache claim. Retain exact commands/build logs beside
reports when using results to choose a subsequent investigation.

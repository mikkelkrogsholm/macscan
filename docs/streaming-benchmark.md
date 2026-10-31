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

## Bounded next-batch prefetch experiment

`--compare-prefetch --run-small` or the separately authorized
`--compare-prefetch --allow-large 100000` adds a candidate mode. Each mode gets
one warmup and three measured repeats over the same owned fixture. Repeat mode
order alternates (serial/candidate/fallback, then fallback/candidate/serial).
The unprofiled warmup remains the saved SQL parity reference. The example
requires nonempty worker metadata batches on the 100k fixture; worker calls
that only observe EOF do not establish meaningful prefetch.

This developer route uses the same bulk syscall, returned-attribute parser,
per-entry stat fallback, guarded walk, streaming store and finalization. It
creates a helper lazily only inside the explicit prefetch profiling API.
Normal scan options, APIs, policy fingerprints and threading remain serial.
There is no UI or CLI product activation.

One helper owns at most one outstanding job/result globally. A job transfers
an existing 64 KiB raw buffer and one owned `F_DUPFD_CLOEXEC` descriptor; the
helper never receives a `DIR` pointer. The duplicate shares the bulk directory
offset, and the coordinator does not enumerate that directory again until it
receives the result. Descriptor-relative metadata/open operations do not change
that offset. The helper closes its descriptor before sending the result.
Parsing and all entry emission remain on the coordinator, preserving preorder,
contiguous subtrees and hardlink ownership. No child is preopened for prefetch.

Batches containing nonregular entries or missing metadata stay serial. This
prevents a parent batch containing directories from reserving the global slot
with a speculative EOF while its large descendants are processed. If a retained
parent result nevertheless exists, descendants use ordinary synchronous bulk
calls until it is consumed. The occupied slot remains reserved even after the
worker finishes, so neither queued results nor raw buffers accumulate with
fanout/depth. There is one transferred raw buffer, no second decoded queue,
and at most one extra descriptor (below the two-buffer/two-descriptor limit).
The existing per-directory buffer/decoded rows are unchanged bounds.

The first bulk call is synchronous, preserving unsupported-first-call
rewind/readdir behavior without outstanding helper work. Later helper failures
remain enumeration/coverage errors. Bulk/directory drop drains outstanding
work before closing the directory, and the profiling scope joins its helper
on completion, error, stop or unwind. A synchronous kernel call cannot be
forcibly preempted; cancellation can wait for that call to return.

`prefetch_worker_overlapping` reports actual helper syscall calls and elapsed
wall time separately from coordinator syscall timings. `prefetch_records` and
`prefetch_nonempty_batches` distinguish useful metadata from EOF. Helper time
can overlap row processing and Parquet flush callbacks: never add it to the
exclusive top-level sum or subtract it from walk-only time. Candidate
`walk_uninstrumented_residual_ms` is null when helper calls occur; serial
residual semantics remain unchanged. Total wall, exclusive accounting,
validation timing and sampled/lifetime resource definitions are unchanged.

The focused example tests exercise actual nonempty worker batches, complete
saved SQL parity, forced fallback, stop/publication and a post-traversal budget
failure. Engine tests cover the occupied-result slot, owned-descriptor syscall
errors, later-bulk error behavior, emit error, cancellation and panic drainage.
No performance threshold is inferred from these functional tests.

A warm APFS/M3 Max/macOS 26.6.2 run (minimum OS 14 compile only) produced:

| Owned fixture | Serial median ms | Prefetch median ms | Fallback median ms | Prefetch wall reduction |
|---|---:|---:|---:|---:|
| 2,009 paths | 208.150 | 206.541 | 206.316 | 0.77% |
| 100,089 paths | 629.224 | 627.916 | 834.439 | 0.21% |

All nine measured runs per fixture had zero saved SQL mismatches. Each large
candidate run prefetched 79,871 records in 128 nonempty batches. Walk-only wall
fell from 231–242 ms to 159–165 ms, while watcher join rose from 10–17 ms to
81–90 ms; complete wall remained almost unchanged. The predeclared 20% large
end-to-end reduction was not achieved. Small regression, exact parity and
resource limits passed: complete large benchmark process lifetime RSS was
318,832,640 bytes; sampled FD maximum 97; sampled stage maximum 12,724,021
logical bytes. These are one warm-fixture experiment and the resource
measurement limits above apply. There is no production activation or inference
that isolated traversal gains satisfy the end-to-end threshold.

MAIN's decision for this delivery is **NO GO to production parallelization
(issue #10)** because the predeclared end-to-end threshold failed. The
candidate remains developer-only. Cold-cache behavior, other hardware and
other real workloads are unmeasured; their performance remains unknown.

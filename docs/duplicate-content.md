# Duplicate DATA-content clarification (#15)

This deliverable is a standalone design experiment, **not product activation**.
It changes no scanner, UI, snapshot, provider policy or existing measurement.
MAIN's decision is **LIMITED GO to a later conditional implementation issue** for
this staged, verified DATA-content pipeline, subject to final delivery review.
This is not product activation or general local/provider admission. Before that
later implementation starts, independently demonstrate admitted scope plus
provider/dataless/unknown-residency refusal before content opens, with OFF
confirmed before any target path call. MAIN owns that follow-up issue; this
worker creates none. The #12 no-go remains binding. No deletion, automatic choice, upload, permanent hash
cache or arbitrary-path scan is implemented.

## Evidence and interpretation

A filename or size match is only a candidate. The prototype groups metadata by
logical size, selects one representative per `(device, inode)` to separate
hardlink aliases, filters with bounded prefix/suffix SHA-256, then computes full
SHA-256 only for colliding candidates. Partial hashes never verify equality.
`verified-DATA-observation` means matching full SHA-256 DATA hashes and successful
identity/size/time revalidation at observation time. SHA-256 is cryptographic
content evidence, not a byte-by-byte comparison or proof against every possible
hash collision. Resource forks, extended attributes and other metadata can differ.

Algorithm ID: `sha256-prefix-suffix-4096/full-sha256/v1`. The partial filter reads
up to 4096 prefix bytes and up to 4096 nonoverlapping suffix bytes. Empty files
have valid full empty-DATA hashes and contribute exactly zero logical duplicate
content bytes. Groups report decimal integer sizes and the full digest.

`logical_duplicate_content_bytes = sum((representative_count - 1) * DATA_size)`.
This counts repeated logical DATA among independently identified representatives,
including a native clone when its DATA verifies equal. Hardlink aliases and the
native clone creation relation are separate outputs. None establishes unique,
private, physical, reclaimable, deletion-saving or allocated-space bytes. A clone
relationship alone never verifies byte equality or physical savings.

The observed pipeline follows principles from MAIN's review of pinned
[Czkawka duplicate core](https://github.com/qarmin/czkawka/blob/d776b0a8559fc3db1429fa29b91608026e97cd01/czkawka_core/src/tools/duplicate/core.rs):
size grouping, hardlink filtering, prefix/suffix filtering before full hashing,
progress/Stop and versioned cache keys. No source or dependency is copied. No
space-saving interpretation from another application's counters is adopted.

## Owned-fixture boundary and observation races

Run `python3 scripts/duplicate-content-probe.py --owned-local-fixtures`, optionally
followed by `--self-test` (the same bounded scenario suite). Default invocation
performs no work. Every other argument combination fails before fixture creation;
there is no root/path argument. One unique directory is created in the fixed OS
temporary area (`/private/tmp` on this Mac). No parent directory is enumerated.
Only registered single-component synthetic names are accessed or removed. The
fixture has no traversed child directories or caller-controlled ancestors.

Content files are opened relative to the retained root descriptor with
`O_NOFOLLOW`; regular-file and root identity checks surround reads. `O_NONBLOCK`
also prevents a substituted special file from blocking on open before its
regular-file check. Pre/post `fstat` and no-follow path metadata must agree with
captured device, inode, size, `mtime_ns` and `ctime_ns`. A final pass revalidates
all captured entries, including skipped sizes and hardlink aliases. The resource
fork fixture validation also uses checked regular-file descriptors. Known fixture
creation changes are completed before capturing hash-candidate metadata.

This is a live, non-atomic observation. Mutation/replacement after the final
check remains possible, and metadata checks are not a snapshot or a permanence
guarantee. An adversarial change that evades metadata detection is not ruled out.
The fixed-depth owned fixture does not certify arbitrary user paths. A future
product must reuse its root/device/raw-path protections rather than treating
this prototype as an ancestor-traversal or provider certification.

Any error, instability, Stop or exhausted budget makes the run unverified, with
no returned verified partial group and a null logical duplicate count. JSON
stderr progress reports the current verification stage and actual hash DATA
bytes returned by reads; the JSON result includes stage, opened files and bytes.
Short reads are counted before failure. A forced partial final read tests exact
budget accounting. Resource-fork validation bytes are separately reported and
included in the invocation's total content-read cap; hash totals exclude them.
Writes, metadata calls and bytes potentially read internally by the filesystem
are not represented as measured content bytes.

The invocation has at most 32 registered names/candidate metadata entries, a
64 KiB maximum read chunk, an 8388608-byte actual content-read cap and a 20-second
between-operation deadline. Hash input buffers are bounded; only small generated
fixture payloads are held in memory. Stop/deadline/budget checks run before new
files and each hash read chunk. SIGINT/SIGTERM request Stop; injected controls
test the same next-read refusal. An in-flight synchronous filesystem call is not
preemptible. Cleanup ignores normal Stop/read budgets, closes descriptors and
unlinks only registered names, then removes the one owned directory. SIGKILL,
process abort or a permanently stuck call cannot promise cleanup.

## Provider boundary and future opt-in design

[Provider metadata findings (#11)](provider-metadata-findings.md) show that
metadata/path operations themselves can contact providers or materialize data.
Neither absence of `SF_DATALESS` nor `MNT_LOCAL` proves non-provider origin or
local residency. This fixture is known local **by construction only**. No real
provider, placeholder or user file is inspected; no real `SF_DATALESS` flag is
set. The injected known-provider/dataless/unknown-residency rejection cases are
explicitly **SIMULATED**, and implement no runtime provider discovery. There is
no no-hydration or no-contact guarantee from this experiment.

Any future product route must establish its materialization policy with a
confirmed OFF guard **before any target path call**, preserve all existing
provider/dataless exclusions and fail closed on unknown residency. OFF is a
necessary guard for that proposed route, not provider certification or a promise
that calls cannot contact/block. Explicit native opt-in and supported admission
must be demonstrated independently before activation; #11 does not provide
blanket admission. The ordinary scanner remains unchanged.

A future opt-in flow would show the exact user-selected areas, admitted/excluded
coverage and content-read budget before consent. No target content opens before
admission, metadata eligibility and consent. Progress must separate metadata,
partial filter, full verification and unverified/error/Stop states, exposing
actual bytes and Stop. Results must distinguish metadata candidates, partial
candidates, full DATA evidence, aliases, clone relations and unknown coverage.
The prototype does not implement that product flow.

No persistent or session hash cache exists here. If a later implementation uses
even temporary session memoization, invalidate on identity, size, mtime, ctime,
algorithm/version or capture change, and revalidate; path alone is never a key.
Show saved historical scan time separately from the current hashing time; never
rewrite old measurements to include, exclude or imply later content work.

## Reproduction and observed results

On 2026-10-31, the owned-fixture run completed on the installed Darwin/APFS
machine with native `clonefileat` success (no copying fallback). The independent
`b` file's 35-byte resource fork was written and read via native descriptor
xattr calls, while `a` had no resource fork (`ENOATTR`). Their DATA hashes still
matched; this demonstrates DATA evidence is not all-fork equivalence.

The focused suite exercised:

| Owned scenario | Observed result |
| --- | --- |
| Independent `a`, `b`, native `clone`, each 196608 DATA bytes | One full-SHA256 DATA group; logical duplicate content 393216 bytes |
| Hardlink `alias` of `a` | Alias output; no extra representative/count |
| Same-size different DATA | Rejected by partial filter |
| Same prefix/suffix, different middle | Reached full hash, rejected |
| Two zero-length files | Full empty-DATA group; zero logical duplicate bytes |
| Unique-size file | No content open |
| Symlink | Unverified before any content open/read |
| Mutation during full hashing; replacement of current pathname | Unverified identity/time revalidation; no partial group |
| Stop; 81937-byte per-case budget; injected EIO; deadline | Unverified, null counter; budget case returned exactly 81937 bytes |
| SIMULATED provider, dataless, unknown residency | Unverified with zero content opens/reads; not live provider evidence |

Each observed successful run returned 1499153 hash DATA bytes plus 35 fixture
resource-fork validation bytes, exactly 1499188 total content bytes. Both owned
fixture paths reported by the final runs were checked absent after cleanup.

Successful native clone/fork outcomes are explicitly reported; unavailable
native facilities report unknown, never a fake pass. Unsupported environments
cannot reproduce the native evidence above solely through synthetic controls.

Commands used (all from the repository root):

```sh
python3 scripts/duplicate-content-probe.py
python3 scripts/duplicate-content-probe.py --owned-local-fixtures /tmp
python3 scripts/duplicate-content-probe.py --owned-local-fixtures --self-test
python3 scripts/duplicate-content-probe.py --owned-local-fixtures
python3 -c "import ast, pathlib; ast.parse(pathlib.Path('scripts/duplicate-content-probe.py').read_text())"
git diff --check
```

Default exits 0 with no fixture work; the path-bearing invocation exits 2 before
fixture creation. Both opted-in commands execute the same focused suite. Each
successful run reports cleanup completed and its exact removed fixture path;
checking that owned path confirms removal. No app/Rust build or real provider
experiment is part of this clarification. MAIN must review actual outcomes
before making any future implementation decision.

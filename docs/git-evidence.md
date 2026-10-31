# Optional current Git evidence clarification (#14)

This is an owned synthetic experiment and a proposed later acceptance boundary.
It changes no product behavior. Git evidence is a separate, explicit current-state
operation: the existing metadata-only scan and historical snapshot remain intact.
A local ignore rule never establishes that data is disposable, regenerable, safe
to delete, or reclaimable.

MAIN's decision: **NO GO to product activation of this direct Git-subprocess
approach on arbitrary live repositories**. #14's clarification is complete; the
default product stays unchanged. Owned stationary fixtures support the semantics
and corrected read-mostly behavior, but cannot enforce the admitted Git-data read
boundary under config/pointer/ignore races. Reconsideration requires evidence of
read confinement and no-write behavior for live, sparse and concurrent repositories,
under separate scope and authorization. No implementation issue is warranted by
this no-go decision.

## Exact experiment and read boundary

Run `python3 scripts/git-evidence-probe.py --owned-local-fixtures`. No arguments
performs no work; all other arguments fail without fixture creation. There is no
arbitrary root/path argument. The probe creates one unique temporary parent,
initializes only repositories it owns, and removes that parent in `finally`.
Python standard library and `/usr/bin/git` are its only dependencies. It never
queries this checkout, a real user repository, remote URLs, or network services.

Fixture setup is explicitly separate: `init`, synthetic writes, `add`, `commit`,
`worktree add`, `update-index` and `sparse-checkout` construct the cases. These
commands may read/write the probe's own synthetic contents and Git objects.
The JSON lists every exact argv/cwd/time/result and identifies setup mutation
command indices. No shell interprets arguments.

Each current check performs the following:

1. Validate a literal relative candidate (maximum 1024 bytes/eight components).
   Use the explicitly known owned repository; never search upward for `.git`.
2. Read the `.git` marker, optional `commondir`, config(s), HEAD and index.
   Normalize pointers inside the owned fixture parent and reject symlink path
   components or external pointers. Linked worktree common data is admitted only
   within that parent. No recursive worktree/submodule discovery occurs.
3. From the separate nonrepository cwd run
   `git config --no-includes --file <explicit-config> --null --list`.
   Reject malformed config, includes, unknown keys/extensions, duplicate keys in
   a file, conflicting common/worktree values, ambiguous booleans and unsupported
   format values. This intentionally accepts only a small
   known configuration subset. The optional worktree config is also checked.
4. Check candidate ancestors by path metadata: any nested `.git` marker,
   including a broken symlink, yields unverified/review. Preflight ordinary
   `.gitignore` rule files and Git `info/exclude` files with a 64 KiB read cap;
   reject symlinks/nonregular/oversize/unreadable rules. These are ignore-rule
   reads, not reads of ordinary candidate file contents.
5. Run index-first, with explicit `--git-dir` and `--work-tree`, from that
   repository's cwd:
   `git --literal-pathspecs ... ls-files --cached --stage --sparse -z -- <candidate>`.
   Then run the same command without the pathspec for bounded ancestor evidence.
   Parse NUL records and stage/mode/path separately. Component-aware, conservative
   case-folded matching protects tracked descendants and ancestors; a name prefix
   alone does not match. A related gitlink (160000), sparse directory (040000),
   conflict stage or malformed output means unverified/review.
6. Only without tracked/ambiguous evidence run
   `git ... check-ignore --stdin -z --verbose`, passing exactly one NUL-terminated
   literal candidate. Parse source/line/pattern/path as four NUL fields. Negated
   `!` rules mean not locally ignored. Exit 1 with no record means no local rule;
   other errors or missing/malformed records mean unverified.

The ellipsis above is the exact override list in `OVERRIDES` and in the JSON:
`--no-optional-locks --no-lazy-fetch`, `core.fsmonitor=false`,
`core.hooksPath=/dev/null`, `core.excludesFile=/dev/null`,
`core.untrackedCache=false`, `maintenance.auto=false`, and `gc.auto=0`.
`--literal-pathspecs` applies to `ls-files` only: `check-ignore --stdin` already
uses literal paths and rejects that global flag as unsupported pathspec magic.
There is no `status`, `diff`, content hashing, ordinary data read, submodule
recursion or lazy object fetch in the control command sequence.

The subprocess environment is an allowlist: system PATH, C locale,
`GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`,
`GIT_TERMINAL_PROMPT=0`, `GIT_OPTIONAL_LOCKS=0`, `GIT_NO_LAZY_FETCH=1`.
Inherited Git overrides, HOME/global config, SSH and shell settings are omitted;
HOME/CODEX_HOME are never reassigned. Excluding global/system excludes is
intentional: this is **local ignore evidence**, not complete global Git status.
The harness requires Git >=2.36 (boolean `core.fsmonitor=false` semantics), but
only the installed Apple Git 2.50.1 was exercised. Older flag support is not
certified; command errors fail closed. Python >=3.9 is needed.

Each check permits at most eight subprocesses and five seconds of subprocess
wall time in total; each child has a two-second ceiling and 64 KiB per output
stream. Both streams are drained with bounded retained bytes; timeout/output
failure kills the child process group and waits for the child. Fixed fixtures
permit at most 32 checks. Git control-file reads cap at 64 KiB; Git-data-only
before/after fingerprinting caps 1024 entries and 2 MiB per file. These bounds
can produce unverified even for an otherwise valid large repository. They do
not turn blocking filesystem syscalls/process creation into preemptible calls.

## Observed results

Local evidence: `evidence/issue-14/probe.json` (ignored, exact commands/results),
`default.log`, `invalid-arguments.log`, `verification.log`, and
`sparse-exploration.md`. No Rust/app build or native app launch ran here.

| Owned synthetic situation | Observed current evidence |
| --- | --- |
| Tracked `target/tracked.txt` under ignored build-name directory | `target` stays tracked/review; ignore cannot override index truth |
| Valuable untracked `target/valuable.db` | Locally ignored by `target/`; always review, no safety claim |
| `!cache/keep.txt`, unrelated `ignoredness`, literal `literal*.txt` | Not locally ignored/review; negation, prefix and literal semantics respected |
| Tracked literal `literal[1].txt`, case variation `TARGET` | Tracked/review; conservative case-fold protection |
| Linked worktree, common data inside own parent | Tracked/review, no external Git-data discovery |
| Nested repo, broken nested `.git` symlink, submodule ancestor | Unverified/review; no recursion |
| Real sparse index and conflicted index stage | Unverified/review from actual index mode/stage |
| Missing index with HEAD retained, corrupted index, missing marker | Unverified; absence is never evidence of an untracked path |
| Malformed `.git`, config or commondir; unknown extension | Unverified |
| External include/pointer, symlink/oversize ignore rules | Rejected before evidence query; external targets not opened |
| Index chmod 000 | Actual native PermissionError, unverified; injected fallback is only used on hosts bypassing permissions |
| Synthetic sleeping/output-flood children | Timeout/output cap observed, child killed/reaped |

The final run asserts before/after Git-data hashes, lengths, modes and mtimes
are equal during the stationary control phase. Only owned Git data is hashed;
ordinary file contents are never hashed. An installed post-index-change hook
never creates its marker. The controlled valuable data is preserved by construction:
there is no deletion/cleanup action on any candidate before whole-fixture cleanup.
All launched subprocesses are terminal and the owned fixture parent is removed.

A deliberately separate fixture mutation then adds `target/valuable.db` to the
index between two current checks: first locally ignored/review, then tracked/review,
each with its own increasing `checked_at`. The illustrative historical metadata
object retains its original `captured_at`, metadata and null Git evidence byte
for byte. This tests the proposed separation, not product snapshot integration.

One exploratory failure matters: `ls-files --cached --stage` **without**
`--sparse` expanded a sparse index and created/touched owned Git tree objects,
even with optional locks disabled. Adding `--sparse` to both index commands
prevented that modification in the final stationary test. The failed exploratory
variant is not used by the final probe. Read-shaped Git commands and
`--no-optional-locks` alone are insufficient proof of no repository writes.

## Acceptance boundary if separately reconsidered

The future user flow should explicitly offer “Check current Git evidence” for
one selected candidate. Before running it, disclose the repository, ignore-rule
reads and every separate Git/common-data area requiring permission (including
linked-worktree data outside a selected root). No automatic discovery outside
that consent, default capture, background checks, caching or cleanup action.
Show current check time alongside the saved scan date; never rewrite historical
metadata. Every output remains reviewable and retains error/unknown reasons.

A later implementation should retain the exact bounded index-first semantics,
protective ancestors, literal/NUL parsing, safe subprocess environment and separate
tracked/local-ignore/unverified states demonstrated here, with focused tests for
this matrix. Global ignore exclusion must be visible. Missing index remains
unverified: this design deliberately does not read HEAD objects to infer tracking.
A proposal that needs broader Git/config/object reads needs separate acceptance.

This is **not a Git sandbox**. Preflight followed by Git has races: config,
pointers, index or ignore rules can change after validation; an external include
introduced then could be read by Git. No race-free, adversarial or concurrent-state
certification follows from these owned stationary fixtures. No real user path is
authorized by running this example. A production implementation must resolve or
explicitly gate that boundary before offering the operation on arbitrary live
repositories. Do not promote this script into a user-path helper as it stands.

Sources reviewed by MAIN: [Git check-ignore](https://git-scm.com/docs/git-check-ignore),
[Git ls-files](https://git-scm.com/docs/git-ls-files),
[Git options](https://git-scm.com/docs/git), and
[Git configuration](https://git-scm.com/docs/git-config).
[dua's pinned implementation](https://github.com/Byron/dua-cli/blob/bc03feddbdd946c16dd234a28b377d43c0688597/src/clean/git.rs)
informs protective index/ancestor principles only; no code or dependency copied.

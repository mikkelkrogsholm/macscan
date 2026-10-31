# Saved-metadata category rules (context-1)

`entries` and `before_entries` derive categories when read. Parquet and manifests
are never rewritten. `saved_category` is the old snapshot label and is not
contextual evidence. No manifest contents, Git index/ignore contents, or live
scan-target metadata are read. Classification does not establish reclaimable
space, deletion safety, authorship, or which process wrote a file.

Every normal navigation/inspect row and desktop node exposes `category`,
`category_rule_id`, `category_rule_version`, `category_reason`,
`category_coverage`, and `category_evidence_source`. SQL exposes the same fields.
The evidence source identifies saved entry/marker IDs, not current files.
Numeric metrics and comparison coverage retain their existing meanings.

## Deterministic precedence

Raw `name_bytes` and saved `kind` provide exact identity; lossy display strings
are never proof. Only saved regular files count as markers; symlinks and
same-named directories do not. Only directories establish inherited scopes.

For an entry, use the deepest applicable ancestor directory (including itself).
A verified `node_dependencies` scope continues through ordinary package manifests
inside `node_modules`; those manifests do not cancel dependency classification.
Nearer explicit build/dependency directories still override it.

Directory contexts, in precedence order:

1. A regular `Cargo.toml` or `package.json` child establishes a project boundary
   (`project_boundary`, unclassified). This stops inheritance from an outer
   cache/build/dependency directory. It also wins if the project itself has a
   misleading name such as `target`.
2. `target` beside a regular `Cargo.toml`: `rust_target`, probable `build_output`.
3. `node_modules` beside a regular `package.json`: `node_dependencies`, probable
   `dependencies`.
4. Unmarked `target` or `node_modules`: `unverified_name`, unclassified. This
   also blocks an outer label from appearing to prove a nested name.
5. Directory name hints: `.next`, `build`, `DerivedData` (`build_hint`);
   `.venv` (`dependency_hint`); `.cache`, `Caches`, `__pycache__` (`cache_hint`);
   `.git` (`git_hint`); `.codex` (`agent_hint`). These remain unverified hints.

A regular file named `Docker.raw` has a direct `docker_hint` (`docker_disk`),
which overrides directory inheritance. No matching scope means `unknown`,
`unclassified`. Rule IDs plus `context-1` identify this exact contract.

`category_coverage` is `partial` when the entry, evidence scope, or its saved
parent has incomplete coverage. A missing marker under partial enumeration
remains unverified: absence is not proof. `context_unavailable` denotes the
scan root without applicable internal evidence or an unverified scope without
its parent metadata. `complete` only describes the available historical
context; even complete name hints are unverified. A root's outside parent and
siblings are never inferred from its path or read live.

Older readable snapshots missing optional `name_bytes`, `path_key`, or
`enumeration_complete` columns receive null context and `context_unavailable`;
missing raw identity is never reconstructed from display strings. Required
storage/metric columns still follow the existing Parquet compatibility contract.

Native navigation first chooses entry IDs using saved byte metrics and filters,
then calls the shared `entries_classified(ids)` table macro. Its materialized
`picked` CTE fences contextual joins to those entries. SQL point consumers can
also use `SELECT * FROM entries_classified([123])`. General category aggregation
through `entries` can inspect all saved metadata within the existing memory
budget; arbitrary SQL is not an unlimited-memory interface.

The implementation uses lazy SQL views of candidate scopes and saved regular
markers, with bounded DuckDB memory. It neither creates a Rust tree copy nor
recursively materializes all ancestor pairs. Human sunburst stays capped at
320 nodes/three levels; agent output stays at 8 KiB with default ten-row pages.

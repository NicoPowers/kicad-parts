# Legacy catalog import oracle

`manifest.json` is a versioned, deterministic inventory of the checked-in legacy catalog inputs and generated `parts.sqlite` projection. Schema version 2 adds host-independent path scopes, fail-closed path resolution, and explicit SQLite `NULL` cells. The source CSVs under `database/` and CAD assets under `symbols/`, `footprints/`, and `3d-models/` remain read-only; this fixture does not copy, rewrite, or execute them. SQLite is opened read-only and its tables, columns, rows, byte hash, and logical-content hash are recorded separately from the CSV source manifest.

The oracle intentionally records both exact byte SHA-256 hashes and semantic SHA-256 hashes. CSV semantics preserve header order, row order, every unknown/category-specific column, every value, and blank fields while ignoring quoting and line-ending serialization. SQLite semantics preserve `NULL` separately from an empty string. KiCad s-expression semantics ignore formatting whitespace outside strings. STEP/STP semantics normalize line endings and trailing horizontal whitespace only.

Reference classification is lexical and host-independent: POSIX absolute, Windows drive/rooted, UNC, variable-based, and relative references receive the same scope on every host. Local resolution rejects absolute paths, drive prefixes, traversal, and unsafe components before filesystem probing. Fixture discovery rejects symbolic links, Windows reparse points, and unsupported entry types rather than following them outside the repository.

Generate or verify it from the repository root:

```text
cargo run -p legacy-import-fixture -- write .
cargo run -p legacy-import-fixture -- check .
```

The checked manifest records known defects as diagnostics. In particular, CSV discovery produces 21 SQLite category tables while the KiCad database descriptor intentionally exposes 19; `mec` and `pcb` are not descriptor entries. The older shell generator also omits `pcb`. Unresolved local and embedded 3D references remain visible rather than being discarded.

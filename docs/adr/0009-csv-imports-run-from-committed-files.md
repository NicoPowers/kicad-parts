# CSV imports run only from committed files, and each run records what the CSV said

The import refuses files that differ from the last commit, because "commit first" is one rule that is easy to explain. It also refuses a checkout older than the latest import run. Each run records the files it read with their hashes, the bytes of every row, and where each part ended up. So the latest run always holds what the CSV says now, and `tooling/catalog-import/src/zero_loss.rs` can rebuild the CSV from it. Decided 2026-10-10.

## Consequences

- `--allow-uncommitted-changes` and `allow_older_checkout` override these checks, and the run's audit entry records the override.
- Each import states the revisions it expects. If the Catalog changed in the meantime, the server answers `409` and the import stops.

Mulch records: mx-c5e787, mx-64c42a, mx-08bf3f.

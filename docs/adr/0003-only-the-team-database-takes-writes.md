# Only the team server's database takes writes

Each database has one job. The team server's PocketBase database is the only one people change. Each desktop keeps a copy that applies the server's numbered Changes in order. Files that KiCad reads, such as a database library, are generated again from those records. We sync records, never SQLite files, and no SQLite file has two writers.

## Consequences

- The desktop copy is in `crates/catalog/src/replica.rs`. It ignores a Change it has already applied, so nothing is missed or applied twice.
- The generated KiCad database library isn't built yet (kicad-parts-f397).

Mulch records: mx-95dadd.

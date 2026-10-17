# Only the team server's database takes writes

Each database has one job. The team server's PocketBase database is the only one people change. Each desktop keeps a copy that applies the server's numbered Changes in order. Files that KiCad reads, such as a database library, are generated again from those records. We sync records, never SQLite files, and no SQLite file has two writers.

## Consequences

- The desktop copy is in `crates/catalog/src/replica.rs`. It ignores a Change it has already applied, so nothing is missed or applied twice.
- The desktop writes the KiCad database library with `crates/kicad-libraries`, from one thread, holding a lock file so no second writer runs. It changes the SQLite database in place, in one transaction, so each table KiCad reads is whole. KiCad's background refresh reads one table at a time, so during one refresh a moving row can show in two groups or in none; its links still resolve, because KiCad then looks in every table.

Mulch records: mx-95dadd.

# PocketBase is the only team backend

A team shares one PocketBase server. We chose PocketBase for its sign-in, realtime updates, file storage, migrations, server hooks and backups. Team rules run in our own server-side routes in `infra/pocketbase/pb_hooks/`. We don't build an adapter for another backend, or a harness that compares backends.

## Consequences

- Each PocketBase version is pinned with its checksum in `infra/versions.lock`, and every upgrade is rehearsed first.
- Backups go to S3-compatible storage, and tests prove a restore into a blank server. PocketBase 0.38 can't restore in place on Windows, so a restore unpacks the backup into an empty `pb_data` folder.
- Integration tests use real, disposable PocketBase and MinIO processes, because sign-in, realtime updates, migrations and restores can't be faked.

Mulch records: mx-c3f845, mx-cbdbb4.

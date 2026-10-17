# A revision means a CAD change, and revisions are archived, never deleted

Old products must keep their exact parts while new designs use the newest. So a part's revision is one exact set of symbol, footprint and 3D model revisions, and only a change to the CAD adds one. Part details and manufacturer parts belong to the IPN, so every revision of a part shares them. A bad revision is archived, never deleted. Decided 2026-10-10.

## Consequences

- A part always keeps at least one revision that isn't archived, so it always has a current revision. Unarchiving a revision needs a reason.
- CAD item revisions are numbered like a part's revisions. Going back to earlier content adds a new revision, so the current revision is always the newest. Each file's bytes are stored once, under their SHA-256.
- A different IPN is always a different part. "Revision" never means a product version.

Mulch records: mx-5e5fb1, mx-6aafdb, mx-17f3e4, mx-0aa85b.

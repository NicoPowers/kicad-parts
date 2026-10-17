# Each revision of a part gets its own KiCad library entry

KiCad updates a schematic's parts by the library entry name stored in it. If that name had no revision, updating an old design would pull in newer CAD. So entries are named with their revision, such as `RES-0000-0470_v2`, while the IPN field and the BOM keep the plain IPN. Older revisions go in an older-revisions library, and archived revisions in a hidden library. Decided 2026-10-10. Not built yet (kicad-parts-f397).

## Consequences

- Each category is one group in KiCad's chooser. Footprints use KiCad's package groups, such as `Package_SO`, because one package serves many categories.
- Generated `.kicad_dbl` files turn on `globally_unique_keys` from the first release, because turning it on later breaks links in existing schematics.

Mulch records: mx-f75a28, mx-544722.

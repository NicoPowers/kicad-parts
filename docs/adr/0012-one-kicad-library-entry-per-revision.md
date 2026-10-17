# Each revision of a part gets its own KiCad library entry

KiCad updates a schematic's parts by the library entry name stored in it. If that name had no revision, updating an old design would pull in newer CAD. So entries are named with their revision, such as `RES-0000-0470_v2`, while the IPN field and the BOM keep the plain IPN. Decided 2026-10-10.

Every entry is in one database library, `team-parts`, with `globally_unique_keys` on. A placed part's link is then `team-parts:RES-0000-0470_v2`, which names no group, and KiCad looks for it in every table of the library. Current revisions are grouped by category, and older and archived revisions have one group each. Changed 2026-10-17, when it was built (kicad-parts-f397): the first plan put archived revisions in a hidden library, but KiCad can hide only a whole library, and moving a revision to another library would break its link. So archived revisions stay in the chooser, under "Archived revisions".

## Consequences

- Each category is one group in KiCad's chooser. Footprints use KiCad's package groups, such as `Package_SO`, because one package serves many categories.
- `globally_unique_keys` is on from the first release, because turning it on later breaks links in existing schematics. The nickname `team-parts` is in every placed link, so it never changes.
- One KiCad group has one list of fields, so the older and archived groups hold only the fields every category shares. Updating a design from those groups keeps its category fields, such as Tolerance, as they were.
- KiCad reads the `.kicad_dbl` only when it starts, so the older and archived groups are always listed, even when empty. A row moving into a group KiCad has not loaded would not be found.
- Symbols, footprints and 3D models get revision names too, such as `R_US_v2`, and a written revision's files never change. The code is in `crates/kicad-libraries`.

Mulch records: mx-f75a28, mx-544722, mx-a97f8c, mx-363dcb, mx-95fdbc, mx-2b1688.

# Each revision of a part gets its own KiCad library entry

KiCad updates a schematic's parts by the library entry name stored in it. If that name had no revision, updating an old design would pull in newer CAD. So entries are named with their revision, such as `RES-0000-0470_v2`, while the IPN field and the BOM keep the plain IPN. Decided 2026-10-10.

Every entry of a team is in one database library, named with the team's code, such as `MA-Parts` (ADR 0015). Its `globally_unique_keys` setting is on. A placed part's link is then `MA-Parts:RES-0000-0470_v2`, which names no group, and KiCad looks for it in every table of the library. Current revisions are grouped by category, and older and archived revisions have one group each. Changed 2026-10-17, when it was built (kicad-parts-f397): the first plan put archived revisions in a hidden library, but KiCad can hide only a whole library, and moving a revision to another library would break its link. So archived revisions stay in the chooser, under "Archived revisions".

## Consequences

- Each category is one group in KiCad's chooser. Footprints use KiCad's package groups, such as `MA-Resistor_SMD`, because one package serves many categories. Each 3D model goes beside its footprint library, linked by a path relative to it, so the folder can move and boards carry no user name.
- `globally_unique_keys` is on from the first release, because turning it on later breaks links in existing schematics.
- KiCad reads every symbol in a file by the file's version, so each hidden symbol library holds one category's symbols saved in one KiCad version, such as `MA-res-20241209`. Symbols newer than KiCad 10 reads are left out and named.
- One KiCad group has one list of fields, so the older and archived groups hold only the fields every category shares. Updating a design from those groups keeps its category fields, such as Tolerance, as they were.
- KiCad reads the `.kicad_dbl` only when it starts, so every group is listed, even when empty. A row moving into a group KiCad has not loaded would not be found.
- Symbols, footprints and 3D models get revision names too, such as `R_US_v2`. A written revision's files never change, and no entry is ever taken away. The code is in `crates/kicad-libraries`.

Mulch records: mx-f75a28, mx-544722, mx-a97f8c, mx-363dcb, mx-95fdbc, mx-2b1688, mx-f43bfc.

# Category guess clues

`parts.json` keeps what the category guess needs from the 178 parts of the old
CSV catalog. That catalog was deleted on 2026-10-26 (kicad-parts-e257). The
file lets `crates/catalog/tests/category_guess_on_the_catalog.rs` keep
measuring the guess on the same parts (ADR 0010).

It holds:

- `categories`: the 21 categories of the old catalog, by code, with the part
  details fields each one had.
- `parts`: one entry per part, in the old catalog's order. Each has its IPN,
  its category, and the clues a downloaded part would give: the reference
  letter of its symbol, its footprints, description, manufacturer and MPN.

A symbol from KiCad's own libraries gave no reference letter, because those
libraries were not in the repository. So 25 parts have a blank `reference`,
as they did when the guess was first measured.

The file was written once from the CSV files and is never written again.
Change it only to fix a clue that was read wrong, and say why in the commit.

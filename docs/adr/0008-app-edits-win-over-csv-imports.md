# Edits made in the app win over CSV imports

A CSV import never overwrites an edit made in the app, so a later import can't quietly undo someone's work. A part's details follow the CSV only while its current details came from an import, and the same goes for its CAD. When the CSV disagrees with an edit, the import records an import problem. A person can then accept the CSV's values field by field in a CSV review. Decided 2026-10-10.

## Consequences

- An import never sends a CAD item made in the app. If the CSV's file differs, each part using it gets the import problem `csv-cad-differs-from-edited-cad-item`, and a CSV review can only keep the app's version.

Mulch records: mx-ee29d9, mx-4854f0, mx-6a908f.

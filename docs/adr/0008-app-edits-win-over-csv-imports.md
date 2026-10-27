# Edits made in the app win over CSV imports

Superseded by ADR 0024 (2026-10-26): the CSV import is gone, so nothing imports over an edit.

A CSV import never overwrites an edit made in the app, so a later import can't quietly undo someone's work. A part's details follow the CSV only while its current details came from an import, and the same goes for its CAD. When the CSV disagrees with an edit, the import records an import problem. A person can then accept the CSV's values field by field in a CSV review. Decided 2026-10-10.

## Consequences

- An import never sends a CAD item made in the app. If the CSV's file differs, each part using it gets the import problem `csv-cad-differs-from-edited-cad-item`, and a CSV review can only keep the app's version.
- A CSV row whose IPN belongs to a part added in the app is that part when it names one of the part's MPNs, or is an exact copy of its part details and CAD (kicad-parts-3b72). Otherwise nothing shows they are one part: the import records `ipn-given-to-another-part`, never takes the row as new values, and no CSV review can take it. The message never says they differ, because the row may be the same part with an MPN written another way, so it gives both ways out. A missing or placeholder MPN (`N/A`, `TBD`) matches nothing. A wrong guess would overwrite a part; a wrong flag only asks someone to check.
- A shared MPN is trusted, even a generic one such as `LM358` in two packages. The CSV review of the row's differences is the guard there.

Mulch records: mx-ee29d9, mx-4854f0, mx-6a908f, mx-45b723.

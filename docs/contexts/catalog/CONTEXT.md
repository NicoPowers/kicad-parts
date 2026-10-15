# Catalog

The parts your team designs with and builds. For each part, the Catalog records what it is, its number, how its details changed, and which exact CAD items it uses.

## Language

### Parts and numbers

**Part**:
Anything with an IPN that the team designs with or builds, including bare boards.
_Avoid_: Managed Part, component, item, row, entry

**IPN**:
A part's permanent number, like `RES-0000-0470`, which is never changed or reused. The prefix says what kind of part it is, the middle number groups variants of one design, and the last four digits pick the variant.
_Avoid_: part number on its own, SKU, IPN Base, Variation

**IPN prefix**:
The letters at the start of an IPN, like `RES`. It shows which number range the IPN came from, not which category the part is in.
_Avoid_: IPN Family, category code, CCC

**Excluded from BOM**:
A setting for a part that appears in designs but is never bought or assembled, such as a fiducial. It comes only from KiCad's own marks in the CAD a revision uses: a symbol marked not in the BOM, or every footprint choice marked excluded from the BOM. So it belongs to the revision: each new revision works it out again, and an existing revision keeps its own.
_Avoid_: Off-BOM, no-BOM, virtual part

### Details and history

**Part details**:
What a part is: its category, description, value, datasheet and manufacturer parts. Every revision of the part shares them, so changing them is recorded in the history but adds no revision.
_Avoid_: semantic change, attributes

**Supplier data**:
Supplier part numbers and prices. They belong to Procurement and never create a revision; the Catalog only keeps them as part of what each CSV import read.
_Avoid_: Operational Fact

**Revision**:
A saved copy of the exact symbol, footprint and 3D model a part uses, or of one CAD item's content, which never changes afterwards. Only a change to CAD adds a revision, and only if the new revision can replace the old one; otherwise it is a new part with its own IPN.
_Avoid_: Part Revision, Asset Revision, version, edit

**Current revision**:
The newest revision of a part or CAD item that is not archived.
_Avoid_: Head, latest

**Archived**:
A revision hidden from KiCad's part chooser and from the app, so nobody new picks it. Designs that already use it still find it, nothing is deleted, and archiving can be undone, with a reason.
_Avoid_: withdrawn, deleted, retired, obsolete, restore

**Change**:
One saved update with a reason, such as an import, an edit, an archive or a CSV review. It can change part details and add or archive revisions of several parts and CAD items at once.
_Avoid_: Change Set, commit, batch, transaction

**Audit entry**:
A record of who did something to the Catalog, when, and why.
_Avoid_: Audit Event, log entry

### Grouping and sourcing

**Category**:
A group of similar parts that share the same fields, such as resistors. It is the only grouping people see: KiCad's part chooser shows one group per category, and the symbols the team makes are grouped the same way. Footprints are grouped by package instead, because one package serves many categories, and each 3D model goes with its footprint. Each CSV file of the CSV catalog makes one category. A part's category is part of its details, so changing it never changes the part's IPN. A supplier's category is only a suggestion until someone confirms it.
_Avoid_: Catalog Category, type, class, IPN prefix

**Remembered match**:
A clue that a person confirmed for a category when they chose a part's category, such as its footprint family or manufacturer. The category guess for new parts uses them; nothing else makes one.
_Avoid_: alias, mapping, learned rule

**Manufacturer part**:
A manufacturer and their part number (MPN) approved for an IPN. A part can have several, the first is the main one, and every revision of the part shares them.
_Avoid_: Approved Manufacturer Part, substitute, alternate, AVL entry

### CAD items

**CAD item**:
A symbol, footprint or 3D model that parts use. It keeps its identity when its content changes.
_Avoid_: Catalog Asset, asset, file, library item

**Installed library**:
A KiCad library that someone added to KiCad on their own computer, listed in their KiCad library tables. Teammates may not have it. The app only reads it.
_Avoid_: user library, personal library, third-party library

**Source**:
Where a revision's data came from, such as a CSV file and row, the file someone chose in the app, the files someone downloaded for a new part and the site they came from, or the file in our libraries and the commit it was read from. The app records that commit; the team server checks the bytes it stores, not the commit.
_Avoid_: Provenance, origin

**License**:
The terms for using a CAD item. "Unknown, needs review" is a real answer, never a blank.
_Avoid_: License Declaration, license disposition

### Trust

**Verification**:
Proof that one exact revision was checked for a purpose, such as a test build or a design review.
_Avoid_: Qualification, approval, sign-off

**Protection**:
A setting that blocks changes to a part or CAD item. To change it, someone must remove the protection and give a reason.
_Avoid_: Engineering Protection, lock, freeze

### Importing

**CSV catalog**:
The original CSV files and CAD libraries this product imports from. Engineers may still draw symbols and footprints in KiCad in its libraries; the library check finds them.
_Avoid_: Legacy Catalog, old database, source of truth

**Import problem**:
Something an import could not handle, such as a missing symbol. The import records it instead of dropping it.
_Avoid_: Import Diagnostic, warning, error

**Library check**:
A comparison of the CSV catalog's libraries with the Catalog: what is in them but not in the Catalog yet, and which 3D model links work only on one computer or name a missing file. Its errors stop an import, and Add part or Edit part, until they are fixed; old problems of footprints the Catalog already has unchanged are only clean-up.
_Avoid_: lint, audit, validation

**CSV review**:
A person's answer to new CSV values that an import did not apply because the part, or a symbol or footprint it uses, was edited in this app: they accept some of the CSV's values and keep the rest, or keep all the current values. It covers what the CSV said, so the app does not ask again about the same CSV values; it asks again when the CSV changes.
_Avoid_: merge, sync, override

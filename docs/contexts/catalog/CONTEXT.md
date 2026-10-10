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
A setting for a part that appears in designs but is never bought or assembled, such as a fiducial.
_Avoid_: Off-BOM, no-BOM, virtual part

### Details and history

**Part details**:
What a part is and how you design with it: its description, value, symbol, footprint, datasheet and manufacturer parts. Only a change to part details creates a revision.
_Avoid_: semantic change, attributes

**Supplier data**:
Supplier part numbers and prices. They belong to Procurement and never create a revision; the Catalog only keeps them as part of what each CSV import read.
_Avoid_: Operational Fact

**Revision**:
A saved copy of a part's details or of a CAD item, which never changes afterwards. Every change, including restoring old details, adds a new revision.
_Avoid_: Part Revision, Asset Revision, version, edit

**Current revision**:
The newest revision of a part or CAD item.
_Avoid_: Head, latest

**Change**:
One saved update with a reason, such as an import or an edit. It can add revisions to several parts and CAD items at once.
_Avoid_: Change Set, commit, batch, transaction

**Audit entry**:
A record of who did something to the Catalog, when, and why.
_Avoid_: Audit Event, log entry

### Grouping and sourcing

**Category**:
A group of similar parts that share the same fields, such as chip resistors. A supplier's category is only a suggestion until someone confirms it.
_Avoid_: Catalog Category, type, class, IPN prefix

**Manufacturer part**:
A manufacturer and their part number (MPN) approved for an IPN. A part can have several, and the first is the main one.
_Avoid_: Approved Manufacturer Part, substitute, alternate, AVL entry

### CAD items

**CAD item**:
A symbol, footprint or 3D model that parts use. It keeps its identity when its content changes.
_Avoid_: Catalog Asset, asset, file, library item

**Source**:
Where a revision's data came from, such as a CSV file and row.
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
The original CSV files and CAD libraries this product imports from. Nobody edits them.
_Avoid_: Legacy Catalog, old database, source of truth

**Import problem**:
Something an import could not handle, such as a missing symbol. The import records it instead of dropping it.
_Avoid_: Import Diagnostic, warning, error

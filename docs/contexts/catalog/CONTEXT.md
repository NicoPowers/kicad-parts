# Catalog

The parts a team designs with and produces. The Catalog says what each part is, how it is numbered, how its record has changed, and which exact CAD it uses.

## Language

### Parts and numbering

**Managed Part**:
Anything the team designs with or produces that has its own IPN. That includes bare boards and Off-BOM parts.
_Avoid_: component, item, row, entry, part record

**IPN**:
The Internal Part Number: the permanent identifier of a Managed Part, written `CCC-NNNN-VVVV`. It is never changed or reused.
_Avoid_: part number (on its own), SKU

**IPN Family**:
The category code at the start of an IPN, such as `RES` or `CAP`. It names the numbering namespace an IPN was issued from, independent of how the part is classified.
_Avoid_: category, group, CCC group

**IPN Base**:
An IPN Family plus its sequence number, such as `RES-0000`. It is shared by variations of one design that usually have a single datasheet.
_Avoid_: family, series

**Variation**:
The last four digits of an IPN. They tell apart the Managed Parts under one IPN Base, for example by value, package, or product version. A different Variation is always a different Managed Part.
_Avoid_: revision

**Off-BOM**:
A property of a Managed Part that appears in designs but is never purchased or assembled, such as a fiducial or mounting hole.
_Avoid_: no-BOM, virtual part

### Revisions and changes

**Part Revision**:
An immutable, complete snapshot of a Managed Part's catalog data. An edit creates a successor revision. Restoring older data also creates a new revision rather than reviving the old one.
_Avoid_: version, edit, history entry

**Head**:
The current Part Revision of a Managed Part.
_Avoid_: latest, current version

**Change Set**:
A reasoned group of Part Revisions and Asset Revisions published together, such as one import or one edit.
_Avoid_: commit, batch, transaction

**Audit Event**:
A lasting record of an action on the Catalog: who acted, when, on what, and why.
_Avoid_: log entry

**Semantic Change**:
A change to what a Managed Part is or how it is designed with. Only Semantic Changes create Part Revisions.
_Avoid_: update

**Operational Fact**:
Information that changes without changing the part itself, such as supplier offers, prices, stock, scans, and drafts. It never creates a Part Revision.
_Avoid_: part data, attribute

### Classification

**Catalog Category**:
A node in the versioned catalog taxonomy that defines which fields a Managed Part must have. It is independent of the part's IPN Family.
_Avoid_: category code, type, class

**Supplier Category Mapping**:
A reviewed link from a supplier's own category to a Catalog Category. A supplier category never reclassifies a part by itself.
_Avoid_: auto-classification

### Sourcing

**Approved Manufacturer Part**:
A manufacturer and manufacturer part number (MPN) approved to satisfy one IPN. The primary MPN is simply the first one.
_Avoid_: substitute, alternate, AVL entry

### CAD assets

**Catalog Asset**:
A long-lived design artifact a Managed Part uses, such as a schematic symbol, footprint, or 3D model. It keeps its identity across changes to its exact content.
_Avoid_: file, library item

**Asset Revision**:
An immutable version of a Catalog Asset, together with its Provenance and License Declaration. Part Revisions bind to exact Asset Revisions.
_Avoid_: asset version

**Asset Blob**:
The exact bytes of an Asset Revision, identified by their content. Moving or copying an Asset Blob never creates a revision.
_Avoid_: file copy, attachment

**Provenance**:
Where a Part Revision or Asset Revision came from: its source, its origin, and the exact input it was derived from.
_Avoid_: source info, origin data

**License Declaration**:
The stated terms under which an Asset Revision may be used and shared. "Undeclared, needs review" is an explicit declaration, never a blank.
_Avoid_: license disposition, license info

### Trust

**Qualification**:
Immutable evidence that an exact revision was verified for a purpose, such as a test build or an engineering review.
_Avoid_: approval, sign-off

**Engineering Protection**:
An auditable policy that restricts changes to a Managed Part or Catalog Asset. Changing a protected item takes an explicit, reasoned unprotect.
_Avoid_: lock, freeze

### Legacy

**Legacy Catalog**:
The part CSV files, substitutes list, and local CAD libraries from before this product. They are read-only inputs to import and are never edited.
_Avoid_: old database, source of truth

**Import Diagnostic**:
A recorded problem found while importing the Legacy Catalog, such as an unresolved reference. It is kept rather than dropped.
_Avoid_: warning, error

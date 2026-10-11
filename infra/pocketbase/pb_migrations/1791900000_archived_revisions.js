/// <reference path="../pb_data/types.d.ts" />

// Archive a part revision instead of restoring it (seed kicad-parts-5bea).
//
// An archived revision is hidden from KiCad and the app, so nobody new
// picks it; designs that already use it still find it, and nothing is
// deleted. Archive state is never written onto the revision (revisions
// never change): each archive or unarchive is a new, unchangeable record in
// catalog_archive_entries, and a revision's state is its newest entry. The
// part's current revision is the newest revision that is not archived.
//
// Data saved by 1791800000_shared_part_details.js stays valid, so nothing
// is refused. The undo refuses any archive entry, which the older version
// cannot read.
migrate((app) => {
  const KINDS = ["csv-import", "edit", "archive", "unarchive"];
  const parts = app.findCollectionByNameOrId("catalog_parts");
  const revisions = app.findCollectionByNameOrId("catalog_part_revisions");
  const changes = app.findCollectionByNameOrId("catalog_changes");

  const entries = new Collection({
    type: "base",
    name: "catalog_archive_entries",
    listRule: "@request.auth.id != ''",
    viewRule: "@request.auth.id != ''",
    createRule: null,
    updateRule: null,
    deleteRule: null,
    fields: [
      { name: "part", type: "relation", collectionId: parts.id, maxSelect: 1, required: true },
      { name: "part_revision", type: "relation", collectionId: revisions.id, maxSelect: 1, required: true },
      { name: "revision_number", type: "number", required: true, min: 1, onlyInt: true },
      // true for an archive, false for an unarchive.
      { name: "archived", type: "bool" },
      { name: "change", type: "relation", collectionId: changes.id, maxSelect: 1, required: true },
      // The Change's seq, so a revision's newest entry is easy to find.
      { name: "seq", type: "number", required: true, min: 1, onlyInt: true },
      // The part's current revision once this entry is saved.
      { name: "current_revision_after", type: "relation", collectionId: revisions.id, maxSelect: 1, required: true },
      { name: "current_revision_number_after", type: "number", required: true, min: 1, onlyInt: true },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: [
      "CREATE INDEX idx_catalog_archive_entries_part ON catalog_archive_entries (part, seq)",
      "CREATE INDEX idx_catalog_archive_entries_change ON catalog_archive_entries (change)",
    ],
  });
  app.save(entries);

  changes.fields.getByName("kind").values = KINDS;
  app.save(changes);
}, (app) => {
  if (app.findAllRecords("catalog_archive_entries").length > 0) {
    throw new Error(
      "this team server has archived revisions, which the older version cannot read. Nothing was " +
        "changed. Undo this change only on a team server with no archive entries, or start again " +
        "with an empty data folder (see docs/operations/TRY_THE_TEAM_CATALOG.md)",
    );
  }
  app.delete(app.findCollectionByNameOrId("catalog_archive_entries"));
  const changes = app.findCollectionByNameOrId("catalog_changes");
  changes.fields.getByName("kind").values = ["csv-import", "edit"];
  app.save(changes);
});

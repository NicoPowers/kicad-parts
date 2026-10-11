/// <reference path="../pb_data/types.d.ts" />

// Keep part details apart from part revisions (seed kicad-parts-7556).
//
// A part's details (its describing fields and manufacturer parts) are
// shared by every revision. Each change saves the next numbered copy in the
// new collection catalog_part_details and adds no revision; the part points
// to its current details. A part revision now holds only the CAD the part
// uses: its Symbol and Footprint fields, the CAD item revisions they bind,
// and Excluded from BOM.
//
// Part revisions saved before this change mix details and CAD in one
// document and cannot be split without guessing, so, like
// 1791700000_cad_item_revision_numbers.js, this migration refuses to start
// on such data rather than change it; see
// docs/operations/TRY_THE_TEAM_CATALOG.md. Its undo likewise refuses data
// with saved part details.
//
// The Change kinds become csv-import and edit (restores were removed in
// kicad-parts-9296).
migrate((app) => {
  const KINDS = ["csv-import", "edit"];
  if (app.findAllRecords("catalog_part_revisions").length > 0) {
    throw new Error(
      "this team server's data was saved before part details were kept apart from revisions, " +
        "and it cannot be upgraded. Nothing was changed. Start again with an empty data folder " +
        "(see docs/operations/TRY_THE_TEAM_CATALOG.md)",
    );
  }
  const DOCUMENT = 1000000;
  const parts = app.findCollectionByNameOrId("catalog_parts");
  const changes = app.findCollectionByNameOrId("catalog_changes");

  // One saved copy of a part's details, numbered from 1 per part, never
  // changed once written. Its snapshot is canonical JSON whose SHA-256 is
  // its identity, so it is a text field.
  const details = new Collection({
    type: "base",
    name: "catalog_part_details",
    listRule: "@request.auth.id != ''",
    viewRule: "@request.auth.id != ''",
    createRule: null,
    updateRule: null,
    deleteRule: null,
    fields: [
      { name: "part", type: "relation", collectionId: parts.id, maxSelect: 1, required: true },
      { name: "number", type: "number", required: true, min: 1, onlyInt: true },
      { name: "change", type: "relation", collectionId: changes.id, maxSelect: 1, required: true },
      { name: "snapshot", type: "text", required: true, max: DOCUMENT },
      {
        name: "snapshot_sha256",
        type: "text",
        required: true,
        min: 64,
        max: 64,
        pattern: "^[0-9a-f]{64}$",
      },
      { name: "source", type: "text", required: true, max: DOCUMENT },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: [
      "CREATE UNIQUE INDEX idx_catalog_part_details_number ON catalog_part_details (part, number)",
      "CREATE INDEX idx_catalog_part_details_change ON catalog_part_details (change)",
    ],
  });
  app.save(details);
  details.fields.add(new RelationField({ name: "predecessor", collectionId: details.id, maxSelect: 1 }));
  app.save(details);

  parts.fields.add(new RelationField({ name: "current_details", collectionId: details.id, maxSelect: 1 }));
  parts.fields.add(new NumberField({ name: "current_details_number", required: true, min: 1, onlyInt: true }));
  app.save(parts);

  changes.fields.getByName("kind").values = KINDS;
  app.save(changes);
}, (app) => {
  // The older code reads part details inside part revisions, so, like the
  // upgrade, the undo refuses any data it would leave unreadable.
  if (app.findAllRecords("catalog_part_details").length > 0) {
    throw new Error(
      "this team server keeps part details apart from revisions, which the older version cannot " +
        "read. Nothing was changed. Undo this change only on a team server with no saved part " +
        "details, or start again with an empty data folder (see docs/operations/TRY_THE_TEAM_CATALOG.md)",
    );
  }
  const parts = app.findCollectionByNameOrId("catalog_parts");
  parts.fields.removeByName("current_details");
  parts.fields.removeByName("current_details_number");
  app.save(parts);
  const details = app.findCollectionByNameOrId("catalog_part_details");
  details.fields.removeByName("predecessor");
  app.save(details);
  app.delete(details);
  const changes = app.findCollectionByNameOrId("catalog_changes");
  changes.fields.getByName("kind").values = ["csv-import", "edit", "restore"];
  app.save(changes);
});

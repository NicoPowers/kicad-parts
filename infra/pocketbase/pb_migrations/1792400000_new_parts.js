/// <reference path="../pb_data/types.d.ts" />

// New parts made in the app (seed kicad-parts-9693).
//
// An engineer adds a part from the CAD files they downloaded: one Change of
// kind create holds the new part, its part details 1 and revision 1, and
// revision 1 of each new CAD item (POST /api/catalog/parts, see
// catalog_new_parts.pb.js). Confirming the new part's category also keeps
// the clues that led to it as remembered matches, as a category change in
// an edit does.
//
// Data saved by the earlier migrations stays valid, so nothing is refused.
// The undo refuses a Change of kind create, which the older version cannot
// read.
migrate((app) => {
  const KINDS = ["csv-import", "edit", "archive", "unarchive", "csv-review", "create"];
  const changes = app.findCollectionByNameOrId("catalog_changes");
  changes.fields.getByName("kind").values = KINDS;
  app.save(changes);
}, (app) => {
  if (app.findRecordsByFilter("catalog_changes", "kind = 'create'", "", 1, 0).length > 0) {
    throw new Error(
      "this team server has parts added in the app, which the older version cannot read. Nothing was changed. " +
        "Undo this change only on a team server with no parts added in the app, or start again with an empty " +
        "data folder (see docs/operations/TRY_THE_TEAM_CATALOG.md)",
    );
  }
  const changes = app.findCollectionByNameOrId("catalog_changes");
  changes.fields.getByName("kind").values = ["csv-import", "edit", "archive", "unarchive", "csv-review"];
  app.save(changes);
});

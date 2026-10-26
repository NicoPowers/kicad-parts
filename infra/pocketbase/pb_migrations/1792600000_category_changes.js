/// <reference path="../pb_data/types.d.ts" />

// Categories set up without a CSV file (seed kicad-parts-fbc2).
//
// A superuser sets up a category with POST /api/catalog/categories (see
// catalog_categories.pb.js): one Change of kind category holds the new
// category and its revision 1.
//
// Data saved by the earlier migrations stays valid, so nothing is refused.
// The undo refuses a Change of kind category, which the older version
// cannot read.
migrate((app) => {
  const KINDS = ["csv-import", "edit", "archive", "unarchive", "csv-review", "create", "category"];
  const changes = app.findCollectionByNameOrId("catalog_changes");
  changes.fields.getByName("kind").values = KINDS;
  app.save(changes);
}, (app) => {
  if (app.findRecordsByFilter("catalog_changes", "kind = 'category'", "", 1, 0).length > 0) {
    throw new Error(
      "this team server has categories set up without a CSV file, which the older version cannot read. " +
        "Nothing was changed. Undo this change only on a team server with no such categories, or start again " +
        "with an empty data folder (see docs/operations/TRY_THE_TEAM_CATALOG.md)",
    );
  }
  const changes = app.findCollectionByNameOrId("catalog_changes");
  changes.fields.getByName("kind").values = ["csv-import", "edit", "archive", "unarchive", "csv-review", "create"];
  app.save(changes);
});

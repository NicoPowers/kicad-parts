/// <reference path="../pb_data/types.d.ts" />

// Review CSV changes that were not applied to an edited part (seed
// kicad-parts-75a5).
//
// When a CSV import finds that a part's details (or its symbol or
// footprint) differ from what an edit saved, it keeps the row in an import
// problem and changes nothing. A person then reviews that problem: they take
// some of the CSV's values, or keep theirs. Each review is one new,
// unchangeable record in catalog_csv_reviews, in a Change of kind
// csv-review. A review covers what the CSV said (the SHA-256 of the
// problem's recorded CSV document), so the same CSV content found again by
// later imports needs no new review, while a CSV change does.
//
// Data saved by the earlier migrations stays valid, so nothing is refused.
// The undo refuses any review, which the older version cannot read.
migrate((app) => {
  const KINDS = ["csv-import", "edit", "archive", "unarchive", "csv-review"];
  const parts = app.findCollectionByNameOrId("catalog_parts");
  const problems = app.findCollectionByNameOrId("catalog_import_problems");
  const changes = app.findCollectionByNameOrId("catalog_changes");

  const reviews = new Collection({
    type: "base",
    name: "catalog_csv_reviews",
    listRule: "@request.auth.id != ''",
    viewRule: "@request.auth.id != ''",
    createRule: null,
    updateRule: null,
    deleteRule: null,
    fields: [
      { name: "part", type: "relation", collectionId: parts.id, maxSelect: 1, required: true },
      { name: "problem", type: "relation", collectionId: problems.id, maxSelect: 1, required: true },
      // csv-details-differ-from-edited-details or
      // csv-cad-differs-from-edited-revision.
      { name: "code", type: "text", required: true, max: 100 },
      // SHA-256 of the CSV document the problem recorded: what the CSV said.
      {
        name: "csv_sha256",
        type: "text",
        required: true,
        min: 64,
        max: 64,
        pattern: "^[0-9a-f]{64}$",
      },
      // JSON list of the field names whose CSV values were taken; empty when
      // the person kept their own values.
      { name: "accepted", type: "text", required: true, max: 10000 },
      { name: "change", type: "relation", collectionId: changes.id, maxSelect: 1, required: true },
      // The Change's seq.
      { name: "seq", type: "number", required: true, min: 1, onlyInt: true },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: [
      // One review per part, kind of difference and CSV content.
      "CREATE UNIQUE INDEX idx_catalog_csv_reviews_csv ON catalog_csv_reviews (part, code, csv_sha256)",
      "CREATE UNIQUE INDEX idx_catalog_csv_reviews_change ON catalog_csv_reviews (change)",
    ],
  });
  app.save(reviews);

  changes.fields.getByName("kind").values = KINDS;
  app.save(changes);
}, (app) => {
  if (app.findAllRecords("catalog_csv_reviews").length > 0) {
    throw new Error(
      "this team server has CSV reviews, which the older version cannot read. Nothing was changed. " +
        "Undo this change only on a team server with no CSV reviews, or start again with an empty " +
        "data folder (see docs/operations/TRY_THE_TEAM_CATALOG.md)",
    );
  }
  app.delete(app.findCollectionByNameOrId("catalog_csv_reviews"));
  const changes = app.findCollectionByNameOrId("catalog_changes");
  changes.fields.getByName("kind").values = ["csv-import", "edit", "archive", "unarchive"];
  app.save(changes);
});

/// <reference path="../pb_data/types.d.ts" />

// Categories and remembered matches (seed kicad-parts-ae44).
//
// A category is the only grouping of parts people see. Each change to a category
// saves its next numbered revision in catalog_category_revisions, never
// changed afterwards, in a Change with a reason; a category's current
// revision is its newest. A revision's snapshot is canonical JSON
// (crates/catalog CategoryDetails: code, name, IPN prefix, fields and the
// rules that help guess a new part's category).
//
// A part's category is part of its part details (schema
// kicad-parts/part-details/3), so changing it saves the part's next details
// and never changes its IPN.
//
// Each time a signed-in editor changes a part's category, the clues that led
// to it (the footprint family, the reference letter and the manufacturer)
// are kept in catalog_category_matches, so the guess gets better with use.
//
// Part details saved before this change have no category and cannot be
// given one without guessing, so this migration refuses to start on such
// data rather than change it; see docs/operations/TRY_THE_TEAM_CATALOG.md.
// Its undo likewise refuses data with categories.
migrate((app) => {
  if (app.findAllRecords("catalog_part_details").length > 0) {
    throw new Error(
      "this team server's part details were saved before parts had a category, " +
        "and it cannot be upgraded. Nothing was changed. Start again with an empty data folder " +
        "(see docs/operations/TRY_THE_TEAM_CATALOG.md)",
    );
  }
  const DOCUMENT = 1000000;
  const changes = app.findCollectionByNameOrId("catalog_changes");
  const parts = app.findCollectionByNameOrId("catalog_parts");
  const sha256 = (name) => ({
    name: name,
    type: "text",
    required: true,
    min: 64,
    max: 64,
    pattern: "^[0-9a-f]{64}$",
  });

  // A category's identity: its code, which gives its ID.
  const categories = new Collection({
    type: "base",
    name: "catalog_categories",
    listRule: "@request.auth.id != ''",
    viewRule: "@request.auth.id != ''",
    createRule: null,
    updateRule: null,
    deleteRule: null,
    fields: [
      { name: "code", type: "text", required: true, max: 64 },
      { name: "first_change", type: "relation", collectionId: changes.id, maxSelect: 1, required: true },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: ["CREATE UNIQUE INDEX idx_catalog_categories_code ON catalog_categories (code)"],
  });
  app.save(categories);

  // One revision of a category, numbered from 1 per category.
  const revisions = new Collection({
    type: "base",
    name: "catalog_category_revisions",
    listRule: "@request.auth.id != ''",
    viewRule: "@request.auth.id != ''",
    createRule: null,
    updateRule: null,
    deleteRule: null,
    fields: [
      { name: "category", type: "relation", collectionId: categories.id, maxSelect: 1, required: true },
      { name: "number", type: "number", required: true, min: 1, onlyInt: true },
      { name: "change", type: "relation", collectionId: changes.id, maxSelect: 1, required: true },
      { name: "snapshot", type: "text", required: true, max: DOCUMENT },
      sha256("snapshot_sha256"),
      // Where it came from, such as the team server's setup.
      { name: "source", type: "text", required: true, max: DOCUMENT },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: [
      "CREATE UNIQUE INDEX idx_catalog_category_revisions_number ON catalog_category_revisions (category, number)",
      "CREATE INDEX idx_catalog_category_revisions_change ON catalog_category_revisions (change)",
    ],
  });
  app.save(revisions);
  revisions.fields.add(new RelationField({ name: "predecessor", collectionId: revisions.id, maxSelect: 1 }));
  revisions.addIndex("idx_catalog_category_revisions_predecessor", true, "predecessor", "predecessor != ''");
  app.save(revisions);

  // A person confirmed a category for a part with this clue.
  const matches = new Collection({
    type: "base",
    name: "catalog_category_matches",
    listRule: "@request.auth.id != ''",
    viewRule: "@request.auth.id != ''",
    createRule: null,
    updateRule: null,
    deleteRule: null,
    fields: [
      { name: "category", type: "relation", collectionId: categories.id, maxSelect: 1, required: true },
      {
        name: "clue",
        type: "select",
        required: true,
        maxSelect: 1,
        values: ["supplier-category", "footprint-family", "reference-letter", "manufacturer"],
      },
      { name: "value", type: "text", required: true, max: 500 },
      // The part whose category the person confirmed.
      { name: "part", type: "relation", collectionId: parts.id, maxSelect: 1, required: true },
      { name: "change", type: "relation", collectionId: changes.id, maxSelect: 1, required: true },
      // The Change's seq, actor and actor name: who confirmed it, and when.
      { name: "seq", type: "number", required: true, min: 1, onlyInt: true },
      { name: "actor", type: "text", required: true, max: 200 },
      { name: "actor_name", type: "text", required: true, max: 500 },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: [
      "CREATE UNIQUE INDEX idx_catalog_category_matches_change ON catalog_category_matches (change, clue)",
      "CREATE INDEX idx_catalog_category_matches_clue ON catalog_category_matches (clue, value)",
    ],
  });
  app.save(matches);
}, (app) => {
  // The older code reads part details without a category, so, like the
  // upgrade, the undo refuses any data it would leave unreadable.
  if (app.findAllRecords("catalog_categories").length > 0 || app.findAllRecords("catalog_part_details").length > 0) {
    throw new Error(
      "this team server has categories, which the older version cannot read. Nothing was changed. " +
        "Undo this change only on a team server with no parts, or start again with an empty " +
        "data folder (see docs/operations/TRY_THE_TEAM_CATALOG.md)",
    );
  }
  app.delete(app.findCollectionByNameOrId("catalog_category_matches"));
  app.delete(app.findCollectionByNameOrId("catalog_category_revisions"));
  app.delete(app.findCollectionByNameOrId("catalog_categories"));
});

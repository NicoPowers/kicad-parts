/// <reference path="../pb_data/types.d.ts" />

// At most one successor per CAD item revision (seed kicad-parts-7828).
//
// A unique index on catalog_cad_item_revisions (predecessor), for
// revisions that have one. Importers find each CAD item's current revision
// as the one no revision names as its predecessor
// (catalog_cad_item_revisions_via_predecessor.id = ""); without an index
// SQLite compares every revision with every other, which took about 19 s
// at 6,000 revisions. Unique also means two revisions can never name the
// same predecessor, which would fork a CAD item's history.
//
// The create guard in catalog_write_guards.pb.js already makes each new
// revision the successor of the newest, so data saved through the routes
// fits. Data that does not (written some other way) is refused with a
// plain message and nothing is changed. The undo only drops the index,
// which loses nothing.
migrate((app) => {
  const named = new Set();
  for (const revision of app.findAllRecords("catalog_cad_item_revisions")) {
    const predecessor = revision.getString("predecessor");
    if (predecessor === "") {
      continue;
    }
    if (named.has(predecessor)) {
      throw new Error(
        `this team server has two CAD item revisions that follow revision ${predecessor}, so that ` +
          "CAD item's history forks and it cannot be upgraded. Nothing was changed. Start again " +
          "with an empty data folder (see docs/operations/TRY_THE_TEAM_CATALOG.md)",
      );
    }
    named.add(predecessor);
  }
  const revisions = app.findCollectionByNameOrId("catalog_cad_item_revisions");
  revisions.addIndex("idx_catalog_cad_item_revisions_predecessor", true, "predecessor", "predecessor != ''");
  app.save(revisions);
}, (app) => {
  const revisions = app.findCollectionByNameOrId("catalog_cad_item_revisions");
  revisions.removeIndex("idx_catalog_cad_item_revisions_predecessor");
  app.save(revisions);
});

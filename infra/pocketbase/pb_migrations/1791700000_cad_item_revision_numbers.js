/// <reference path="../pb_data/types.d.ts" />

// Number CAD item revisions like part revisions (seed kicad-parts-54d5).
//
// A CAD item revision's ID now comes from its CAD item and number, not its
// content, and going back to earlier content adds a new revision. So two
// revisions of one CAD item may have the same snapshot hash, and the unique
// index on (cad_item, snapshot_sha256) becomes a plain one. The unique index
// on (cad_item, number) stays.
//
// Data saved before this change names CAD item revisions by content, in
// IDs and in part details, and cannot be upgraded without rewriting records
// that never change. Rather than change them, this migration refuses to
// start on such data; see docs/operations/TRY_THE_TEAM_CATALOG.md.
migrate((app) => {
  const deriveId = (kind, key) => $security.sha256("kicad-parts:" + kind + ":v1\n" + key).substring(0, 15);
  for (const revision of app.findAllRecords("catalog_cad_item_revisions")) {
    const expected = deriveId("cad-item-revision", revision.getString("cad_item") + "\n" + revision.getInt("number"));
    if (revision.id !== expected) {
      throw new Error(
        "this team server's data was saved before CAD item revisions were numbered like part revisions, " +
          "and it cannot be upgraded. Nothing was changed. Start again with an empty data folder " +
          "(see docs/operations/TRY_THE_TEAM_CATALOG.md)",
      );
    }
  }
  const revisions = app.findCollectionByNameOrId("catalog_cad_item_revisions");
  revisions.removeIndex("idx_catalog_cad_item_revisions_hash");
  revisions.addIndex("idx_catalog_cad_item_revisions_content", false, "cad_item, snapshot_sha256", "");
  app.save(revisions);
}, (app) => {
  const revisions = app.findCollectionByNameOrId("catalog_cad_item_revisions");
  const seen = {};
  for (const revision of app.findAllRecords("catalog_cad_item_revisions")) {
    const content = revision.getString("cad_item") + "\n" + revision.getString("snapshot_sha256");
    if (seen[content]) {
      throw new Error(
        `CAD item ${revision.getString("cad_item")} has two revisions with the same content, ` +
          "which the older schema cannot hold; nothing was changed",
      );
    }
    seen[content] = true;
  }
  revisions.removeIndex("idx_catalog_cad_item_revisions_content");
  revisions.addIndex("idx_catalog_cad_item_revisions_hash", true, "cad_item, snapshot_sha256", "");
  app.save(revisions);
});

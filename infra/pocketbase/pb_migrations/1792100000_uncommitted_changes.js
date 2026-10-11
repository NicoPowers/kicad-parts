/// <reference path="../pb_data/types.d.ts" />

// Record when an import was allowed from uncommitted changes (seed
// kicad-parts-7348).
//
// An import from files with changes that are not committed (its source
// commit ends in +dirty) is refused unless the importer is told to allow
// it. The import run's audit entry records that in
// allowed_uncommitted_changes, like allowed_older_checkout.
//
// Data saved by the earlier migrations stays valid, so nothing is refused.
// Runs from uncommitted changes saved before this migration were allowed
// by the older rules and keep false. The undo refuses any run that was
// allowed from uncommitted changes, because the older version would lose
// that record.
migrate((app) => {
  const entries = app.findCollectionByNameOrId("catalog_audit_entries");
  entries.fields.add(new BoolField({ name: "allowed_uncommitted_changes" }));
  app.save(entries);
}, (app) => {
  const allowed = app.findRecordsByFilter(
    "catalog_audit_entries",
    "allowed_uncommitted_changes = true",
    "",
    1,
    0,
  );
  if (allowed.length > 0) {
    throw new Error(
      "this team server has imports that were allowed from uncommitted changes, which the older " +
        "version cannot record. Nothing was changed. Undo this change only on a team server with " +
        "no such imports, or start again with an empty data folder (see " +
        "docs/operations/TRY_THE_TEAM_CATALOG.md)",
    );
  }
  const entries = app.findCollectionByNameOrId("catalog_audit_entries");
  entries.fields.removeByName("allowed_uncommitted_changes");
  app.save(entries);
});

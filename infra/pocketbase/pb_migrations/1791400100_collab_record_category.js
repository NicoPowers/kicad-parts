/// <reference path="../pb_data/types.d.ts" />

// Upgrade rehearsal: adds a field to a populated collection and backfills it
// from existing data without touching any other column, so revisions, the
// change log and the `updated` timestamps survive the migration unchanged.
migrate((app) => {
  const records = app.findCollectionByNameOrId("collab_records");
  records.fields.add(new TextField({ name: "category", max: 100 }));
  app.save(records);

  // The key prefix before the first '-' is the category; keys without one
  // stay uncategorised.
  app
    .db()
    .newQuery(
      "UPDATE collab_records SET category = substr(key, 1, instr(key, '-') - 1) WHERE instr(key, '-') > 1",
    )
    .execute();
}, (app) => {
  const records = app.findCollectionByNameOrId("collab_records");
  records.fields.removeByName("category");
  app.save(records);
});

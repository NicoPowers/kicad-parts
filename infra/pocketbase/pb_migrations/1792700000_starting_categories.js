/// <reference path="../pb_data/types.d.ts" />

// A new team server starts with the starting categories (seed
// kicad-parts-e257), so Add part works on a new team at once. They are
// listed in pb_hooks/catalog_starting_categories.json, which crates/catalog
// keeps in step with STARTING_CATEGORIES. All of them are saved in one
// Change of kind category, the team server's first.
//
// A team server that already saved something is left as it is.
//
// The write guards (catalog_write_guards.pb.js) read committed records,
// which cannot see the tables this migration's transaction made, so the
// records are saved without hooks, as only a migration may.
migrate((app) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  lib.setUpStartingCategories(app.unsafeWithoutHooks());
}, (app) => {
  const changes = app.findRecordsByFilter("catalog_changes", "seq > 0", "seq", 2, 0);
  if (changes.length === 0) {
    return;
  }
  const lib = require(`${__hooks}/catalog_lib.js`);
  const setup = changes[0];
  if (changes.length > 1 || setup.id !== lib.deriveId("change", lib.SETUP_CORRELATION_ID)) {
    throw new Error(
      "this team server has saved more than its starting categories, so they cannot be taken out. " +
        "Nothing was changed.",
    );
  }
  const unguarded = app.unsafeWithoutHooks();
  const owned = (collection) => unguarded.findRecordsByFilter(collection, "change = {:change}", "", 0, 0, {
    change: setup.id,
  });
  for (const revision of owned("catalog_category_revisions")) {
    unguarded.delete(revision);
  }
  for (const category of unguarded.findRecordsByFilter("catalog_categories", "first_change = {:change}", "", 0, 0, {
    change: setup.id,
  })) {
    unguarded.delete(category);
  }
  for (const entry of owned("catalog_audit_entries")) {
    unguarded.delete(entry);
  }
  unguarded.delete(setup);
});

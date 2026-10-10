/// <reference path="../pb_data/types.d.ts" />

// Editing parts in the Catalog (seed kicad-parts-2254).
//
// - Team members marked `can_edit_catalog` may publish edits and restores
//   through POST /api/catalog/edits and POST /api/catalog/restores
//   (pb_hooks/catalog_edits.pb.js). Everyone else who is signed in can only
//   read. Members may still update their own account, but neither their
//   edit access nor their name: the name is shown as the author of every
//   Change, so only a superuser may set it (pb_hooks/catalog_write_guards
//   also refuses names that look like the app's own words).
// - Changes and audit entries record the actor's name as well as their
//   record, because members cannot read other members' accounts.
//
// pb_hooks/catalog_write_guards.pb.js keeps every Catalog record unchanged
// once written; this migration adds no rule that would loosen that.

migrate((app) => {
  const GUARD = " && @request.body.can_edit_catalog:isset = false && @request.body.name:isset = false";
  const users = app.findCollectionByNameOrId("users");
  users.fields.add(new BoolField({ name: "can_edit_catalog" }));
  if (users.updateRule !== null && users.updateRule !== undefined) {
    users.updateRule = "(" + users.updateRule + ")" + GUARD;
  }
  app.save(users);

  for (const name of ["catalog_changes", "catalog_audit_entries"]) {
    const collection = app.findCollectionByNameOrId(name);
    collection.fields.add(new TextField({ name: "actor_name", max: 200 }));
    app.save(collection);
  }
}, (app) => {
  const GUARD = " && @request.body.can_edit_catalog:isset = false && @request.body.name:isset = false";
  for (const name of ["catalog_changes", "catalog_audit_entries"]) {
    const collection = app.findCollectionByNameOrId(name);
    collection.fields.removeByName("actor_name");
    app.save(collection);
  }

  const users = app.findCollectionByNameOrId("users");
  users.fields.removeByName("can_edit_catalog");
  const guard = GUARD;
  // The rule is a Go string pointer, so make it a JS string first.
  const rule = users.updateRule === null || users.updateRule === undefined ? "" : String(users.updateRule);
  if (rule.startsWith("(") && rule.endsWith(")" + guard)) {
    users.updateRule = rule.substring(1, rule.length - guard.length - 1);
  }
  app.save(users);
});

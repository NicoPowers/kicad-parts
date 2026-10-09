/// <reference path="../pb_data/types.d.ts" />

// Editing parts in the Catalog (seed kicad-parts-2254).
//
// - Team members marked `can_edit_catalog` may publish edits and restores
//   through POST /api/catalog/edits and POST /api/catalog/restores
//   (pb_hooks/catalog_edits.pb.js). Everyone else who is signed in can only
//   read. Only a superuser can change the mark: members may still update
//   their own account, but not this field.
// - Changes and audit entries record the actor's name as well as their
//   record, because members cannot read other members' accounts.
//
// pb_hooks/catalog_write_guards.pb.js keeps every Catalog record unchanged
// once written; this migration adds no rule that would loosen that.
migrate((app) => {
  const users = app.findCollectionByNameOrId("users");
  users.fields.add(new BoolField({ name: "can_edit_catalog" }));
  if (users.updateRule !== null && users.updateRule !== undefined) {
    users.updateRule = "(" + users.updateRule + ") && @request.body.can_edit_catalog:isset = false";
  }
  app.save(users);

  for (const name of ["catalog_changes", "catalog_audit_entries"]) {
    const collection = app.findCollectionByNameOrId(name);
    collection.fields.add(new TextField({ name: "actor_name", max: 200 }));
    app.save(collection);
  }
}, (app) => {
  for (const name of ["catalog_changes", "catalog_audit_entries"]) {
    const collection = app.findCollectionByNameOrId(name);
    collection.fields.removeByName("actor_name");
    app.save(collection);
  }

  const users = app.findCollectionByNameOrId("users");
  users.fields.removeByName("can_edit_catalog");
  const guard = " && @request.body.can_edit_catalog:isset = false";
  // The rule is a Go string pointer, so make it a JS string first.
  const rule = users.updateRule === null || users.updateRule === undefined ? "" : String(users.updateRule);
  if (rule.startsWith("(") && rule.endsWith(")" + guard)) {
    users.updateRule = rule.substring(1, rule.length - guard.length - 1);
  }
  app.save(users);
});

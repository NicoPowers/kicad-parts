/// <reference path="../pb_data/types.d.ts" />

// Provisional collaboration schema (seed kicad-parts-b355).
//
// It proves the mechanics the catalog will adopt: a revisioned record that
// only the protected command route may write, an append-only change log with
// a monotonic sequence that clients use as a reconciliation checkpoint, a
// command ledger that makes correlation IDs idempotent, and a record with a
// PocketBase-managed file. It is not the catalog model; kicad-parts-fef6 and
// kicad-parts-2254 own that.
migrate((app) => {
  const users = app.findCollectionByNameOrId("users");
  const signedIn = "@request.auth.id != ''";

  const records = new Collection({
    type: "base",
    name: "collab_records",
    listRule: signedIn,
    viewRule: signedIn,
    // Only superusers may write directly; everyone else goes through
    // POST /api/collab/commands.
    createRule: null,
    updateRule: null,
    deleteRule: null,
    fields: [
      { name: "key", type: "text", required: true, max: 200 },
      { name: "title", type: "text", max: 500 },
      { name: "content", type: "text", max: 20000 },
      { name: "revision", type: "number", required: true, min: 1, onlyInt: true },
      { name: "updated_by", type: "relation", collectionId: users.id, maxSelect: 1 },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
      { name: "updated", type: "autodate", onCreate: true, onUpdate: true },
    ],
    indexes: ["CREATE UNIQUE INDEX idx_collab_records_key ON collab_records (key)"],
  });
  app.save(records);

  const changes = new Collection({
    type: "base",
    name: "collab_changes",
    listRule: signedIn,
    viewRule: signedIn,
    createRule: null,
    updateRule: null,
    deleteRule: null,
    fields: [
      { name: "seq", type: "number", required: true, min: 1, onlyInt: true },
      { name: "record", type: "relation", collectionId: records.id, maxSelect: 1, required: true },
      { name: "record_key", type: "text", required: true, max: 200 },
      { name: "revision", type: "number", required: true, min: 1, onlyInt: true },
      { name: "correlation_id", type: "text", required: true, max: 100 },
      { name: "actor", type: "relation", collectionId: users.id, maxSelect: 1, required: true },
      { name: "title", type: "text", max: 500 },
      { name: "content", type: "text", max: 20000 },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: [
      "CREATE UNIQUE INDEX idx_collab_changes_seq ON collab_changes (seq)",
      "CREATE UNIQUE INDEX idx_collab_changes_record_revision ON collab_changes (record, revision)",
    ],
  });
  app.save(changes);

  const commands = new Collection({
    type: "base",
    name: "collab_commands",
    listRule: "actor = @request.auth.id",
    viewRule: "actor = @request.auth.id",
    createRule: null,
    updateRule: null,
    deleteRule: null,
    fields: [
      { name: "correlation_id", type: "text", required: true, max: 100 },
      { name: "actor", type: "relation", collectionId: users.id, maxSelect: 1, required: true },
      { name: "request_hash", type: "text", required: true, max: 64 },
      { name: "status", type: "number", required: true, min: 200, max: 599, onlyInt: true },
      { name: "result", type: "text", required: true, max: 5000 },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: [
      "CREATE UNIQUE INDEX idx_collab_commands_correlation ON collab_commands (correlation_id)",
    ],
  });
  app.save(commands);

  const attachments = new Collection({
    type: "base",
    name: "collab_attachments",
    listRule: signedIn,
    viewRule: signedIn,
    createRule: signedIn,
    updateRule: null,
    deleteRule: null,
    fields: [
      { name: "record", type: "relation", collectionId: records.id, maxSelect: 1, required: true },
      { name: "label", type: "text", max: 200 },
      {
        name: "file",
        type: "file",
        required: true,
        maxSelect: 1,
        maxSize: 10485760,
        protected: true,
      },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
  });
  app.save(attachments);
}, (app) => {
  for (const name of ["collab_attachments", "collab_commands", "collab_changes", "collab_records"]) {
    app.delete(app.findCollectionByNameOrId(name));
  }
});

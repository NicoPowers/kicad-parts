/// <reference path="../pb_data/types.d.ts" />

// The Catalog's team-authoritative records (seed kicad-parts-fef6).
//
// Terms follow docs/contexts/catalog/CONTEXT.md. Every record is immutable
// once written except a part's current revision pointer. Signed-in members
// may read everything; nobody writes directly. Writes go through the
// protected routes in pb_hooks, which run in one transaction, derive record
// IDs from content (see catalog_lib.js and crates/catalog/src/ids.rs), and
// keep catalog_changes.seq gap-free.
//
// Canonical documents (revision snapshots and sources) are stored in text
// fields, not json fields, so the exact bytes their SHA-256 covers survive a
// round trip.
migrate((app) => {
  const signedIn = "@request.auth.id != ''";
  const readOnly = {
    listRule: signedIn,
    viewRule: signedIn,
    createRule: null,
    updateRule: null,
    deleteRule: null,
  };
  const DOCUMENT = 1000000;
  const sha256 = (name) => ({
    name: name,
    type: "text",
    required: true,
    min: 64,
    max: 64,
    pattern: "^[0-9a-f]{64}$",
  });

  // One saved update with a reason, holding at least one revision, numbered
  // without gaps so replicas can catch up from a checkpoint.
  const changes = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_changes",
    fields: [
      { name: "seq", type: "number", required: true, min: 1, onlyInt: true },
      // A closed set shared with catalog_lib.js (CHANGE_KINDS) and
      // crates/catalog (ChangeKind). These were the first values;
      // 1792600000_category_changes.js sets the current ones.
      { name: "kind", type: "select", required: true, maxSelect: 1, values: ["edit", "restore"] },
      { name: "reason", type: "text", required: true, max: 2000 },
      { name: "correlation_id", type: "text", required: true, max: 100 },
      // "<auth collection>:<record id>", because a superuser is not a users
      // record.
      { name: "actor", type: "text", required: true, max: 200 },
      // JSON of where what it saved came from, such as the files a new
      // part was made from.
      { name: "sources", type: "text", required: true, max: DOCUMENT },
      // JSON counts of what this Change created.
      { name: "summary", type: "text", required: true, max: 10000 },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: [
      "CREATE UNIQUE INDEX idx_catalog_changes_seq ON catalog_changes (seq)",
      "CREATE UNIQUE INDEX idx_catalog_changes_correlation ON catalog_changes (correlation_id)",
    ],
  }));
  app.save(changes);

  // Exact bytes, addressed by SHA-256. The record ID is derived from it.
  const blobs = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_blobs",
    fields: [
      sha256("sha256"),
      { name: "size", type: "number", required: false, min: 0, onlyInt: true },
      {
        name: "file",
        type: "file",
        required: true,
        maxSelect: 1,
        maxSize: 104857600,
        protected: true,
      },
      { name: "first_change", type: "relation", collectionId: changes.id, maxSelect: 1, required: true },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: ["CREATE UNIQUE INDEX idx_catalog_blobs_sha256 ON catalog_blobs (sha256)"],
  }));
  app.save(blobs);

  const cad_items = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_cad_items",
    fields: [
      { name: "kind", type: "select", required: true, maxSelect: 1, values: ["symbol", "footprint", "model-3d"] },
      // LIB:NAME for symbols and footprints, the repository path for 3D models
      { name: "key", type: "text", required: true, max: 500 },
      { name: "first_change", type: "relation", collectionId: changes.id, maxSelect: 1, required: true },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: ["CREATE UNIQUE INDEX idx_catalog_cad_items_kind_key ON catalog_cad_items (kind, key)"],
  }));
  app.save(cad_items);

  const cadItemRevisions = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_cad_item_revisions",
    fields: [
      { name: "cad_item", type: "relation", collectionId: cad_items.id, maxSelect: 1, required: true },
      { name: "number", type: "number", required: true, min: 1, onlyInt: true },
      { name: "change", type: "relation", collectionId: changes.id, maxSelect: 1, required: true },
      { name: "blob", type: "relation", collectionId: blobs.id, maxSelect: 1, required: true },
      // SHA-256 of snapshot, the revision's identity within its CAD item.
      sha256("snapshot_sha256"),
      { name: "snapshot", type: "text", required: true, max: DOCUMENT },
      // Copied from the snapshot so it can be filtered; never blank.
      { name: "license", type: "text", required: true, max: 100 },
      { name: "source", type: "text", required: true, max: DOCUMENT },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: [
      "CREATE UNIQUE INDEX idx_catalog_cad_item_revisions_hash ON catalog_cad_item_revisions (cad_item, snapshot_sha256)",
      "CREATE UNIQUE INDEX idx_catalog_cad_item_revisions_number ON catalog_cad_item_revisions (cad_item, number)",
    ],
  }));
  app.save(cadItemRevisions);
  cadItemRevisions.fields.add(
    new RelationField({ name: "predecessor", collectionId: cadItemRevisions.id, maxSelect: 1 }),
  );
  app.save(cadItemRevisions);

  const parts = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_parts",
    fields: [
      { name: "ipn", type: "text", required: true, max: 13, pattern: "^[A-Z]{3}-[0-9]{4}-[0-9A-Z]{4}$" },
      { name: "ipn_prefix", type: "text", required: true, max: 3 },
      { name: "variant", type: "text", required: true, max: 4 },
      { name: "current_revision_number", type: "number", required: true, min: 1, onlyInt: true },
      { name: "first_change", type: "relation", collectionId: changes.id, maxSelect: 1, required: true },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
      { name: "updated", type: "autodate", onCreate: true, onUpdate: true },
    ],
    indexes: ["CREATE UNIQUE INDEX idx_catalog_parts_ipn ON catalog_parts (ipn)"],
  }));
  app.save(parts);

  const partRevisions = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_part_revisions",
    fields: [
      { name: "part", type: "relation", collectionId: parts.id, maxSelect: 1, required: true },
      { name: "number", type: "number", required: true, min: 1, onlyInt: true },
      { name: "change", type: "relation", collectionId: changes.id, maxSelect: 1, required: true },
      { name: "snapshot", type: "text", required: true, max: DOCUMENT },
      sha256("snapshot_sha256"),
      { name: "source", type: "text", required: true, max: DOCUMENT },
      // The exact CAD item revisions the snapshot binds, for integrity and
      // lookups; the snapshot itself is authoritative.
      { name: "cad_item_revisions", type: "relation", collectionId: cadItemRevisions.id, maxSelect: 999 },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: [
      "CREATE UNIQUE INDEX idx_catalog_part_revisions_number ON catalog_part_revisions (part, number)",
    ],
  }));
  app.save(partRevisions);
  partRevisions.fields.add(
    new RelationField({ name: "predecessor", collectionId: partRevisions.id, maxSelect: 1 }),
  );
  app.save(partRevisions);

  parts.fields.add(new RelationField({ name: "current_revision", collectionId: partRevisions.id, maxSelect: 1 }));
  app.save(parts);

  // One per save: who saved it, when and why, and the Change it made.
  const auditEntries = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_audit_entries",
    fields: [
      { name: "action", type: "text", required: true, max: 100 },
      { name: "actor", type: "text", required: true, max: 200 },
      { name: "change", type: "relation", collectionId: changes.id, maxSelect: 1 },
      { name: "reason", type: "text", required: true, max: 2000 },
      { name: "correlation_id", type: "text", required: true, max: 100 },
      // JSON counts of what the save created.
      { name: "summary", type: "text", required: true, max: 10000 },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
  }));
  app.save(auditEntries);

  // Correlation-ID ledger: a repeated command replays its stored answer.
  const commands = new Collection({
    type: "base",
    name: "catalog_commands",
    listRule: null,
    viewRule: null,
    createRule: null,
    updateRule: null,
    deleteRule: null,
    fields: [
      { name: "correlation_id", type: "text", required: true, max: 100 },
      { name: "actor", type: "text", required: true, max: 200 },
      sha256("request_hash"),
      { name: "status", type: "number", required: true, min: 200, max: 599, onlyInt: true },
      { name: "result", type: "text", required: true, max: 100000 },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: ["CREATE UNIQUE INDEX idx_catalog_commands_correlation ON catalog_commands (correlation_id)"],
  });
  app.save(commands);
}, (app) => {
  // Referrers before what they refer to; self-relations and the current
  // revision pointer first, so no relation blocks a delete.
  const removeField = (name, field) => {
    const collection = app.findCollectionByNameOrId(name);
    collection.fields.removeByName(field);
    app.save(collection);
  };
  const drop = (name) => app.delete(app.findCollectionByNameOrId(name));
  drop("catalog_commands");
  drop("catalog_audit_entries");
  removeField("catalog_parts", "current_revision");
  removeField("catalog_part_revisions", "predecessor");
  drop("catalog_part_revisions");
  drop("catalog_parts");
  removeField("catalog_cad_item_revisions", "predecessor");
  drop("catalog_cad_item_revisions");
  drop("catalog_cad_items");
  drop("catalog_blobs");
  drop("catalog_changes");
});

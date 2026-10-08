/// <reference path="../pb_data/types.d.ts" />

// The Catalog's team-authoritative records (seed kicad-parts-fef6).
//
// Terms follow docs/contexts/catalog/CONTEXT.md. Every record is immutable
// once written except a Managed Part's head pointer. Signed-in members may
// read everything; nobody writes directly. Writes go through the protected
// routes in pb_hooks (today: POST /api/catalog/legacy-imports), which run in
// one transaction, derive record IDs from content (see catalog_lib.js and
// crates/catalog/src/ids.rs), and keep catalog_change_sets.seq gap-free.
//
// Canonical documents (snapshots, declarations, provenance) are stored in
// text fields, not json fields, so the exact bytes their SHA-256 covers
// survive a round trip.
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

  // One reasoned group of revisions published together, numbered without
  // gaps so replicas can catch up from a checkpoint.
  const changeSets = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_change_sets",
    fields: [
      { name: "seq", type: "number", required: true, min: 1, onlyInt: true },
      // A closed set shared with catalog_lib.js (CHANGE_SET_KINDS) and
      // crates/catalog (ChangeSetKind). Only legacy-import is published
      // today; edit and restore are reserved for kicad-parts-2254.
      { name: "kind", type: "select", required: true, maxSelect: 1, values: ["legacy-import", "edit", "restore"] },
      { name: "reason", type: "text", required: true, max: 2000 },
      { name: "correlation_id", type: "text", required: true, max: 100 },
      // "<auth collection>:<record id>", because a superuser is not a users
      // record.
      { name: "actor", type: "text", required: true, max: 200 },
      { name: "source_commit", type: "text", max: 100 },
      // JSON list of every source file read: role, path, byte_sha256,
      // size and, for CSV files, the exact header line.
      { name: "sources", type: "text", required: true, max: DOCUMENT },
      // JSON counts of what this Change Set created.
      { name: "summary", type: "text", required: true, max: 10000 },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: [
      "CREATE UNIQUE INDEX idx_catalog_change_sets_seq ON catalog_change_sets (seq)",
      "CREATE UNIQUE INDEX idx_catalog_change_sets_correlation ON catalog_change_sets (correlation_id)",
    ],
  }));
  app.save(changeSets);

  // Exact bytes, addressed by SHA-256. The record ID is derived from it.
  const blobs = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_asset_blobs",
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
      { name: "first_change_set", type: "relation", collectionId: changeSets.id, maxSelect: 1, required: true },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: ["CREATE UNIQUE INDEX idx_catalog_asset_blobs_sha256 ON catalog_asset_blobs (sha256)"],
  }));
  app.save(blobs);

  const assets = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_assets",
    fields: [
      { name: "kind", type: "select", required: true, maxSelect: 1, values: ["symbol", "footprint", "model-3d"] },
      // LIB:NAME for symbols and footprints, the repository path for 3D models
      { name: "key", type: "text", required: true, max: 500 },
      { name: "first_change_set", type: "relation", collectionId: changeSets.id, maxSelect: 1, required: true },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: ["CREATE UNIQUE INDEX idx_catalog_assets_kind_key ON catalog_assets (kind, key)"],
  }));
  app.save(assets);

  const assetRevisions = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_asset_revisions",
    fields: [
      { name: "asset", type: "relation", collectionId: assets.id, maxSelect: 1, required: true },
      { name: "number", type: "number", required: true, min: 1, onlyInt: true },
      { name: "change_set", type: "relation", collectionId: changeSets.id, maxSelect: 1, required: true },
      { name: "blob", type: "relation", collectionId: blobs.id, maxSelect: 1, required: true },
      // SHA-256 of declaration, the revision's identity within its asset.
      sha256("revision_sha256"),
      { name: "declaration", type: "text", required: true, max: DOCUMENT },
      // Copied from the declaration so it can be filtered; never blank.
      { name: "license_status", type: "text", required: true, max: 100 },
      { name: "provenance", type: "text", required: true, max: DOCUMENT },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: [
      "CREATE UNIQUE INDEX idx_catalog_asset_revisions_hash ON catalog_asset_revisions (asset, revision_sha256)",
      "CREATE UNIQUE INDEX idx_catalog_asset_revisions_number ON catalog_asset_revisions (asset, number)",
    ],
  }));
  app.save(assetRevisions);
  assetRevisions.fields.add(
    new RelationField({ name: "predecessor", collectionId: assetRevisions.id, maxSelect: 1 }),
  );
  app.save(assetRevisions);

  const parts = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_managed_parts",
    fields: [
      { name: "ipn", type: "text", required: true, max: 13, pattern: "^[A-Z]{3}-[0-9]{4}-[0-9A-Z]{4}$" },
      { name: "ipn_family", type: "text", required: true, max: 3 },
      { name: "ipn_base", type: "text", required: true, max: 8 },
      { name: "variation", type: "text", required: true, max: 4 },
      { name: "head_number", type: "number", required: true, min: 1, onlyInt: true },
      { name: "first_change_set", type: "relation", collectionId: changeSets.id, maxSelect: 1, required: true },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
      { name: "updated", type: "autodate", onCreate: true, onUpdate: true },
    ],
    indexes: ["CREATE UNIQUE INDEX idx_catalog_managed_parts_ipn ON catalog_managed_parts (ipn)"],
  }));
  app.save(parts);

  const partRevisions = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_part_revisions",
    fields: [
      { name: "managed_part", type: "relation", collectionId: parts.id, maxSelect: 1, required: true },
      { name: "number", type: "number", required: true, min: 1, onlyInt: true },
      { name: "change_set", type: "relation", collectionId: changeSets.id, maxSelect: 1, required: true },
      { name: "snapshot", type: "text", required: true, max: DOCUMENT },
      sha256("snapshot_sha256"),
      { name: "provenance", type: "text", required: true, max: DOCUMENT },
      // The exact Asset Revisions the snapshot binds, for integrity and
      // lookups; the snapshot itself is authoritative.
      { name: "asset_revisions", type: "relation", collectionId: assetRevisions.id, maxSelect: 999 },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
    indexes: [
      "CREATE UNIQUE INDEX idx_catalog_part_revisions_number ON catalog_part_revisions (managed_part, number)",
    ],
  }));
  app.save(partRevisions);
  partRevisions.fields.add(
    new RelationField({ name: "predecessor", collectionId: partRevisions.id, maxSelect: 1 }),
  );
  app.save(partRevisions);

  parts.fields.add(new RelationField({ name: "head", collectionId: partRevisions.id, maxSelect: 1 }));
  app.save(parts);

  const diagnostics = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_import_diagnostics",
    fields: [
      { name: "code", type: "text", required: true, max: 100 },
      { name: "subject", type: "text", required: true, max: 500 },
      { name: "path", type: "text", required: true, max: 500 },
      { name: "location", type: "text", required: true, max: 2000 },
      { name: "message", type: "text", required: true, max: 10000 },
      { name: "detail", type: "text", required: true, max: DOCUMENT },
      { name: "change_set", type: "relation", collectionId: changeSets.id, maxSelect: 1, required: true },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
  }));
  app.save(diagnostics);

  const auditEvents = new Collection(Object.assign({}, readOnly, {
    type: "base",
    name: "catalog_audit_events",
    fields: [
      { name: "action", type: "text", required: true, max: 100 },
      { name: "actor", type: "text", required: true, max: 200 },
      { name: "change_set", type: "relation", collectionId: changeSets.id, maxSelect: 1, required: true },
      { name: "reason", type: "text", required: true, max: 2000 },
      { name: "correlation_id", type: "text", required: true, max: 100 },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
    ],
  }));
  app.save(auditEvents);

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
  // Referrers before what they refer to; self-relations and the head
  // pointer first, so no relation blocks a delete.
  const removeField = (name, field) => {
    const collection = app.findCollectionByNameOrId(name);
    collection.fields.removeByName(field);
    app.save(collection);
  };
  const drop = (name) => app.delete(app.findCollectionByNameOrId(name));
  drop("catalog_commands");
  drop("catalog_audit_events");
  drop("catalog_import_diagnostics");
  removeField("catalog_managed_parts", "head");
  removeField("catalog_part_revisions", "predecessor");
  drop("catalog_part_revisions");
  drop("catalog_managed_parts");
  removeField("catalog_asset_revisions", "predecessor");
  drop("catalog_asset_revisions");
  drop("catalog_assets");
  drop("catalog_asset_blobs");
  drop("catalog_change_sets");
});

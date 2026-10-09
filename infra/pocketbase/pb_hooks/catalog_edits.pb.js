/// <reference path="../pb_data/types.d.ts" />

// Publishing one new revision of one part (seed kicad-parts-2254). Both
// routes run catalog_lib.js publishPartRevision in one transaction; see
// there for the steps. Team members marked can_edit_catalog and superusers
// may call them; other signed-in members get 403, anyone else 401.
//
// POST /api/catalog/edits: a change to part details.
//
//   { correlationId, reason, ipn, expectedRevision,
//     fields: [{ name, value }],            every field of the current
//                                           revision, same names and order
//     manufacturerParts: [{ manufacturer, mpn }] }   the first is the main
//
// The new revision's CAD references are read from its Symbol and Footprint
// fields and bound to exact CAD item revisions the Catalog already has.
//
// POST /api/catalog/restores: a new revision whose part details are an
// exact copy of an earlier revision's. The earlier revision is never
// changed.
//
//   { correlationId, reason, ipn, expectedRevision, restoreRevision }
//
// Answers (both routes):
//   200 { outcome: "applied", correlationId, ipn, changeId, seq,
//         auditEntryId, revision: { id, number, snapshotSha256 },
//         cadItemRevisions }
//   422 { outcome: "noop", correlationId, ipn, revision, message }
//         the details would not change; nothing is written
//   409 { outcome: "conflict", correlationId, ipn, expectedRevision,
//         currentRevision: { id, number, snapshot, snapshotSha256,
//                            changeKind, reason, actorName, created },
//         message }
//         someone else published a revision first; nothing is written
//   409 { outcome: "correlation_reused", correlationId, message }
//   400 bad input (including a symbol or footprint the Catalog does not
//       have), 401 not signed in, 403 not allowed to edit, 404 no such part
//       or revision.
// Applied and no-op answers are replayed byte for byte for a repeated
// correlation ID (with X-Collab-Replayed: true).
routerAdd(
  "POST",
  "/api/catalog/edits",
  (e) => {
    const lib = require(`${__hooks}/catalog_lib.js`);
    return lib.publishPartRevision(e, lib.CHANGE_KINDS.edit);
  },
  $apis.requireAuth("users", "_superusers"),
);

routerAdd(
  "POST",
  "/api/catalog/restores",
  (e) => {
    const lib = require(`${__hooks}/catalog_lib.js`);
    return lib.publishPartRevision(e, lib.CHANGE_KINDS.restore);
  },
  $apis.requireAuth("users", "_superusers"),
);

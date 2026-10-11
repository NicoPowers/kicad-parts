/// <reference path="../pb_data/types.d.ts" />

// Publishing one new revision of one part (seed kicad-parts-2254). The
// route runs catalog_lib.js publishPartRevision in one transaction; see
// there for the steps. Team members marked can_edit_catalog and superusers
// may call it; other signed-in members get 403, anyone else 401.
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
// Answers:
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
//       have), 401 not signed in, 403 not allowed to edit, 404 no such part.
// Applied and no-op answers are replayed byte for byte for a repeated
// correlation ID (with X-Collab-Replayed: true).
//
// There is no restore route: an earlier revision is never copied back
// (seed kicad-parts-bd76; a bad revision will be archived instead).
routerAdd(
  "POST",
  "/api/catalog/edits",
  (e) => {
    const lib = require(`${__hooks}/catalog_lib.js`);
    return lib.publishPartRevision(e);
  },
  $apis.requireAuth("users", "_superusers"),
);

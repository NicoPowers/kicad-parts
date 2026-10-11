/// <reference path="../pb_data/types.d.ts" />

// Archiving and unarchiving one part revision (seed kicad-parts-5bea). The
// route runs catalog_lib.js publishArchive in one transaction; see there for
// the steps. Team members marked can_edit_catalog and superusers may call
// it; other signed-in members get 403, anyone else 401.
//
// POST /api/catalog/archives
//
//   { correlationId, reason, ipn, revision, archived }
//
// archived is true to archive the revision, false to unarchive it. An
// archived revision is hidden from KiCad and the app, so nobody new picks
// it; designs that already use it keep it, and nothing is deleted. The
// part's current revision is its newest revision that is not archived.
//
// Answers:
//   200 { outcome: "applied", correlationId, ipn, changeId, seq,
//         auditEntryId, revision, archived,
//         currentRevision: { id, number, snapshotSha256 } }
//   422 { outcome: "noop", correlationId, ipn, message }
//         the revision already has that state; nothing is written
//   409 { outcome: "correlation_reused", correlationId, message }
//   400 bad input, or archiving the only revision that is not archived;
//   401 not signed in, 403 not allowed to edit, 404 no such part or
//   revision.
// Applied and no-op answers are replayed byte for byte for a repeated
// correlation ID (with X-Collab-Replayed: true).
routerAdd(
  "POST",
  "/api/catalog/archives",
  (e) => {
    const lib = require(`${__hooks}/catalog_lib.js`);
    return lib.publishArchive(e);
  },
  $apis.requireAuth("users", "_superusers"),
);

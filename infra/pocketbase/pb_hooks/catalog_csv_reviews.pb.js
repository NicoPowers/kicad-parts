/// <reference path="../pb_data/types.d.ts" />

// Reviewing CSV changes that were not applied to an edited part (seed
// kicad-parts-75a5). The route runs catalog_lib.js publishCsvReview in one
// transaction; see there for the steps. Team members marked
// can_edit_catalog and superusers may call it; other signed-in members get
// 403, anyone else 401.
//
// POST /api/catalog/csv-reviews
//
//   { correlationId, reason, ipn, problem, accept, expected }
//
// problem is the ID of a csv-details-differ-from-edited-details or
// csv-cad-differs-from-edited-revision import problem of the part, found by
// the latest import run of its CSV file. accept lists the fields whose CSV
// values to take: details field names and "Manufacturer parts" for a details
// problem, "Symbol" and "Footprint" for a CAD problem. An empty list keeps
// the part's values. expected is the part's current details number (details
// problem) or current revision number (CAD problem). The values saved come
// from the problem's recorded CSV document, never from the request.
//
// Answers:
//   200 { outcome: "applied", correlationId, ipn, changeId, seq,
//         auditEntryId, accepted, details, revision }
//         details and revision are { id, number, snapshotSha256 } or null
//   422 { outcome: "noop", correlationId, ipn, message }
//         these CSV values were already reviewed; nothing is written
//   409 { outcome: "conflict", correlationId, ipn, message,
//         currentDetails | currentRevision }
//         the part changed after the problem was found or the client read it
//   409 { outcome: "out_of_date", correlationId, ipn, message }
//         a later import run of the CSV file no longer finds the problem
//   409 { outcome: "correlation_reused", correlationId, message }
//   400 bad input: a field the CSV did not change, CAD that matches an
//   archived revision, or a document that cannot be saved;
//   401 not signed in, 403 not allowed to edit, 404 no such part or problem.
// Applied and no-op answers are replayed byte for byte for a repeated
// correlation ID (with X-Collab-Replayed: true).
routerAdd(
  "POST",
  "/api/catalog/csv-reviews",
  (e) => {
    const lib = require(`${__hooks}/catalog_lib.js`);
    return lib.publishCsvReview(e);
  },
  $apis.requireAuth("users", "_superusers"),
);

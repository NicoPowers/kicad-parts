/// <reference path="../pb_data/types.d.ts" />

// Setting up a category without a CSV file (seed kicad-parts-fbc2). The
// route runs catalog_lib.js publishCategory in one transaction; see there
// for the steps. Only superusers may call it: no screen sets up categories
// yet. Test team servers and local-team-server --sample use it, so they
// start without the CSV import.
//
// POST /api/catalog/categories
//
//   { correlationId, reason,
//     category: { schema, code, name, ipn_prefix, fields, rules } }
//
// category is a category revision, as crates/catalog CategoryDetails
// writes it. The server saves it in canonical form as revision 1 of a new
// category, in one Change of kind category.
//
// Answers:
//   200 { outcome: "applied", correlationId, changeId, seq, auditEntryId,
//         category: { id, code, revisionId, snapshotSha256 } }
//   409 { outcome: "correlation_reused", correlationId, message }
//   400 bad input, or a category the Catalog already has; 401 not signed
//       in as a superuser.
// Applied answers are replayed byte for byte for a repeated correlation ID
// (with X-Collab-Replayed: true).
routerAdd(
  "POST",
  "/api/catalog/categories",
  (e) => {
    const lib = require(`${__hooks}/catalog_lib.js`);
    return lib.publishCategory(e);
  },
  $apis.requireSuperuserAuth(),
);

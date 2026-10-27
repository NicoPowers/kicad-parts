/// <reference path="../pb_data/types.d.ts" />

// A team's categories (seeds kicad-parts-fbc2 and kicad-parts-e257).
//
// A new team server starts with the starting categories, set up by
// migration 1792700000_starting_categories.js. So Add part works on a new
// team at once.
//
// After that, only a superuser sets up another category, through the route
// below; no screen does yet. The route runs catalog_lib.js publishCategory
// in one transaction.
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

/// <reference path="../pb_data/types.d.ts" />

// Adding a new part from downloaded CAD files (seed kicad-parts-9693). The
// route runs catalog_lib.js publishCreate in one transaction; see there for
// the steps. Team members marked can_edit_catalog and superusers may call
// it; other signed-in members get 403, anyone else 401.
//
// POST /api/catalog/parts
//
//   { correlationId, reason, ipn,
//     category,                          the code of the part's category
//     fields: [{ name, value }],         every field of the category, in its
//                                        order
//     manufacturerParts: [{ manufacturer, mpn }],   first is main
//     cadFields: [{ name, value }],      Symbol, then Footprint
//     uploads: [{ role, reference, fileName, formatVersion?,
//                 text | base64 }],      files, as for edits
//     from?,                             where the files came from, such as
//                                        SnapEDA
//     files? }                           the names of the downloaded files
//
// One Change of kind create holds the new part with part details 1 and
// revision 1, and revision 1 of each new CAD item the uploads make; the
// server reads and checks every file exactly as it does for an edit (see
// catalog_lib.js preparedUploads) and refuses a file that nothing uses. The
// person confirms the category, so the clues that led to it are kept as
// remembered matches, as for a category change in an edit.
//
// Answers:
//   200 { outcome: "applied", correlationId, ipn, changeId, seq,
//         auditEntryId, details: { id, number, snapshotSha256 },
//         revision: { id, number, snapshotSha256 }, cadItemRevisions,
//         newCadItemRevisions: [{ kind, key, number, id, otherParts }],
//         rememberedMatches: [{ clue, value }] }
//   409 { outcome: "ipn_taken", correlationId, ipn, message, addedByYou }
//         the Catalog already has a part with that IPN; nothing is written.
//         addedByYou is true when the caller added that part in the app
//         (a save whose answer was lost, then changed and sent again)
//   409 { outcome: "correlation_reused", correlationId, message }
//   400 bad input (including a malformed IPN, an IPN whose prefix is not
//       the category's IPN prefix, an unknown category, fields
//       that are not the category's, an unsafe name, a symbol or footprint
//       the Catalog does not have and no file was sent for, a file that is
//       wrong or not used, or a file named like a CAD item the Catalog has
//       but with other content, which would change it for every part that
//       uses it), 401 not signed in, 403 not allowed to edit.
// Applied answers are replayed byte for byte for a repeated correlation ID
// (with X-Collab-Replayed: true); ipn_taken is not stored, so a retry is
// checked again.
//
// A request may be at most 32 MiB, like an edit.
const NEW_PART_BODY_LIMIT = 32 * 1024 * 1024;
routerAdd(
  "POST",
  "/api/catalog/parts",
  (e) => {
    const lib = require(`${__hooks}/catalog_lib.js`);
    return lib.publishCreate(e);
  },
  $apis.bodyLimit(NEW_PART_BODY_LIMIT),
  $apis.requireAuth("users", "_superusers"),
);

/// <reference path="../pb_data/types.d.ts" />

// The team's settings (seed kicad-parts-f397): one record per team server,
// made here with a random ID that names the team on every computer, even if
// the server's address changes. It holds the team code, such as MA, that
// names the team's KiCad libraries (MA-Parts, MA-Resistor_SMD). A footprint
// library's name is saved in every board, so the code is set once, through
// POST /api/team/code (team.pb.js), and never changed by the app.
//
// Signed-in members may read the record; only the route and superusers
// write it. To fix a mistaken code before any board uses it, a superuser
// clears or edits the code field in the PocketBase dashboard. Every member
// then signs out and signs in again, because the app reads the team's
// settings only at sign-in, and deletes their kicad-libraries/<old code>
// folder.
//
// The undo refuses a team server whose team has a code, because the older
// version cannot keep it.
migrate((app) => {
  const settings = new Collection({
    type: "base",
    name: "team_settings",
    listRule: "@request.auth.id != ''",
    viewRule: "@request.auth.id != ''",
    createRule: null,
    updateRule: null,
    deleteRule: null,
    fields: [
      // Blank until someone who can edit the Catalog chooses it.
      { name: "code", type: "text", max: 3, pattern: "^([A-Z][A-Z0-9]{1,2})?$" },
      // Who chose it, as the audit entry names them.
      { name: "code_set_by", type: "text", max: 200 },
      { name: "created", type: "autodate", onCreate: true, onUpdate: false },
      { name: "updated", type: "autodate", onCreate: true, onUpdate: true },
    ],
  });
  app.save(settings);
  app.save(new Record(settings));
}, (app) => {
  const found = app.findRecordsByFilter("team_settings", "code != ''", "", 1, 0);
  if (found.length > 0) {
    throw new Error(
      "this team server's team has a code, which the older version cannot keep. Nothing was changed. " +
        "Undo this change only on a team server whose team has no code, or start again with an empty " +
        "data folder (see docs/operations/TRY_THE_TEAM_CATALOG.md)",
    );
  }
  app.delete(app.findCollectionByNameOrId("team_settings"));
});

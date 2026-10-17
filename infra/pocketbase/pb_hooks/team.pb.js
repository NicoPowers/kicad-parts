/// <reference path="../pb_data/types.d.ts" />

// The team's settings (seed kicad-parts-f397; collection team_settings in
// 1792500000_team_settings.js).
//
// GET /api/team, for any signed-in member:
//   200 { id, code, codeSetBy, editors: [name], unnamedEditors }
//   id names the team on every computer; code is blank until chosen;
//   editors are the names of the members who may choose it, so others know
//   who to ask. Members are shown by name only, never by email:
//   unnamedEditors counts those who may choose it but have no name.
//
// POST /api/team/code, for members marked can_edit_catalog and superusers:
//   { correlationId, code }
//   200 { outcome: "applied", id, code, auditEntryId }
//   200 { outcome: "noop", id, code } the team already has this code
//   409 { outcome: "already_set", id, code, message } the team has another
//       code, which the app never changes: it is in every board
//   400 not a team code (2 or 3 capital letters or digits, starting with a
//   letter); 401 not signed in; 403 not allowed to edit.
routerAdd(
  "GET",
  "/api/team",
  (e) => {
    const settings = $app.findAllRecords("team_settings")[0];
    if (!settings) {
      throw new NotFoundError("this team server has no team settings");
    }
    const lib = require(`${__hooks}/catalog_lib.js`);
    const editors = [];
    let unnamedEditors = 0;
    for (const user of $app.findRecordsByFilter("users", "can_edit_catalog = true", "name", 100, 0)) {
      // The same names a Change shows; a reserved name is no name.
      const name = user.getString("name");
      if (name && !lib.isReservedName(name)) {
        editors.push(name);
      } else {
        unnamedEditors += 1;
      }
    }
    return e.json(200, {
      id: settings.id,
      code: settings.getString("code"),
      codeSetBy: settings.getString("code_set_by"),
      editors: editors,
      unnamedEditors: unnamedEditors,
    });
  },
  $apis.requireAuth("users", "_superusers"),
);

routerAdd(
  "POST",
  "/api/team/code",
  (e) => {
    const lib = require(`${__hooks}/catalog_lib.js`);
    const actor = lib.catalogEditor(e);
    const body = e.requestInfo().body || {};
    const correlationId = body.correlationId;
    if (typeof correlationId !== "string" || !/^[A-Za-z0-9_.:-]{1,100}$/.test(correlationId)) {
      throw new BadRequestError("correlationId is required: letters, digits and _ . : - only");
    }
    const code = body.code;
    if (typeof code !== "string" || !/^[A-Z][A-Z0-9]{1,2}$/.test(code)) {
      throw new BadRequestError(
        "a team code is 2 or 3 capital letters or digits, starting with a letter, such as MA",
      );
    }
    let reply = null;
    $app.runInTransaction((txApp) => {
      const settings = txApp.findAllRecords("team_settings")[0];
      if (!settings) {
        throw new NotFoundError("this team server has no team settings");
      }
      const current = settings.getString("code");
      if (current === code) {
        reply = { status: 200, body: { outcome: "noop", id: settings.id, code: code } };
        return;
      }
      if (current !== "") {
        reply = {
          status: 409,
          body: {
            outcome: "already_set",
            id: settings.id,
            code: current,
            message: `the team code is already ${current}; it is in every board that uses the team's footprints, so it is never changed`,
          },
        };
        return;
      }
      settings.set("code", code);
      settings.set("code_set_by", actor.name);
      txApp.save(settings);
      const auditEntryId = lib.deriveId("audit-entry", correlationId);
      const audit = new Record(txApp.findCollectionByNameOrId("catalog_audit_entries"));
      audit.set("id", auditEntryId);
      audit.set("action", "team.code");
      audit.set("actor", actor.id);
      audit.set("actor_name", actor.name);
      audit.set("reason", `Choose the team code ${code} for the team's KiCad libraries`);
      audit.set("correlation_id", correlationId);
      audit.set("summary", JSON.stringify({ code: code }));
      txApp.save(audit);
      reply = {
        status: 200,
        body: { outcome: "applied", id: settings.id, code: code, auditEntryId: auditEntryId },
      };
    });
    return e.json(reply.status, reply.body);
  },
  $apis.requireAuth("users", "_superusers"),
);

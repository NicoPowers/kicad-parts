/// <reference path="../pb_data/types.d.ts" />

// POST /api/catalog/csv-imports: saves one CSV catalog import. Superusers
// only. The request is built by crates/catalog (CsvImport and
// pocketbase::ImportCommand):
//
//   { correlationId, reason, csvFile, sourceCommit, allowOlderCheckout,
//     sources: [{ role, path, byteSha256, size, headerRaw, rows }],
//     blobs: [{ sha256, size, base64 }],
//     cadItemRevisions: [{ id, cadItemId, kind, key, snapshotSha256,
//                          snapshot, source }],
//     parts: [{ id, ipn, expectedRevision, snapshot, snapshotSha256,
//               source }],
//     problems: [{ id, code, subject, path, location, message, detail }] }
//
// `rows` is given for the two CSV files (catalog-csv and substitutes-csv)
// only: every row's exact bytes, in file order. `expectedRevision` is the
// number of the part's current revision when the importer read the team
// catalog, or 0 for a part it expects to be new. `allowOlderCheckout` says
// the importer was told to import from a checkout older than the team
// catalog's latest imports (the importer checks git ancestry; the server
// cannot), and is recorded on the audit entry.
//
// The server trusts none of the derived values: it re-hashes every blob and
// snapshot, re-derives every record ID, checks that each CSV file's header
// line and rows are the file with its byte hash and that every part's
// source row is one of those rows, and checks that every CAD reference
// names a CAD item revision that is stored or arrives in this request.
//
// Then, in one transaction, it compares the request with what is stored:
// - if any part's current revision is not the one the importer expected,
//   someone changed it in between: 409 conflict and nothing is written;
// - blobs, CAD items, CAD item revisions and import problems that already
//   exist are reused (they are content-addressed), after checking that the
//   stored record's hash or key really matches, so an ID collision is
//   refused, not merged;
// - a new IPN becomes a part with revision 1;
// - a part whose current revision has the same snapshot is unchanged. If its
//   source row changed anyway (for example its supplier data), or the
//   current revision has no CSV source to compare with, an import problem
//   keeps the new row;
// - a part whose part details changed gets a new revision if its current
//   revision came from a CSV import, and otherwise an import problem, so a
//   re-import never overwrites an edit made in the Catalog;
// - a part that the previous import run of this CSV file made, but no row
//   makes now, gets an import problem; nothing is deleted.
//
// A Change is created only when there is at least one new revision. Every
// run that writes anything gets an audit entry, and new import problems
// point to it. A CSV import run's audit entry also records exactly what the
// CSV said (the files read, their byte hashes, each CSV file's header line
// and rows) and the current revision it left each part at. A run is
// recorded whenever it read other files than the previous run of the same
// CSV file, even if it adds nothing else, so the latest run always holds
// what the CSV says now.
//
// Answers:
//   200 { outcome: "applied", changeId, seq, auditEntryId, created, unchanged }
//   200 { outcome: "nothing_changed", auditEntryId, problemsRecorded, unchanged }
//                                          no revision, so no Change; the
//                                          run and its new import problems
//                                          are recorded
//   422 { outcome: "noop", unchanged }    everything is already stored and
//                                          the previous run read the same
//                                          files; nothing is written
//   409 { outcome: "conflict", conflicts: [{ ipn, expectedRevision,
//         currentRevision }], message }   someone changed these parts after
//                                          the importer read them; nothing
//                                          is written
//   409 { outcome: "correlation_reused" } the ID was used for another request
//   409 { outcome: "id_collision", collisions } a stored record has a derived
//                                          ID of this request but other
//                                          content; nothing is written
//   400 bad or inconsistent input, 401/403 not a superuser.
// Answers with status 200 or 422 are stored under the correlation ID and
// replayed byte for byte (with X-Collab-Replayed: true) without writing
// anything. Conflicts are not stored, so a retry is checked again.
routerAdd(
  "POST",
  "/api/catalog/csv-imports",
  (e) => {
    const lib = require(`${__hooks}/catalog_lib.js`);
    const body = e.requestInfo().body || {};
    const actor = "_superusers:" + e.auth.id;
    const actorName = e.auth.getString("email");

    const fail = (message) => {
      throw new BadRequestError(message);
    };
    const text = (object, name, max, required) => {
      const value = object[name];
      if (value === undefined || value === null) {
        if (required) {
          fail(`${name} is required`);
        }
        return "";
      }
      if (typeof value !== "string") {
        fail(`${name} must be a string`);
      }
      if (value.length > max) {
        fail(`${name} is longer than ${max} characters`);
      }
      if (required && value.length === 0) {
        fail(`${name} is required`);
      }
      return value;
    };
    const list = (name) => {
      const value = body[name];
      if (!Array.isArray(value)) {
        fail(`${name} must be a list`);
      }
      return value;
    };
    const isSha = (value) => typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
    const parse = (json, what) => {
      try {
        return JSON.parse(json);
      } catch (_) {
        fail(`${what} is not JSON`);
      }
    };

    const correlationId = text(body, "correlationId", 100, true);
    if (!/^[A-Za-z0-9_.:-]+$/.test(correlationId)) {
      fail("correlationId may only contain letters, digits and _ . : -");
    }
    const reason = text(body, "reason", 2000, true);
    if (reason.trim().length === 0) {
      fail("reason is required");
    }
    const csvFile = text(body, "csvFile", 100, true);
    const sourceCommit = text(body, "sourceCommit", 100, false);
    const DOCUMENT = 1000000;

    const allowOlderCheckout = body.allowOlderCheckout === undefined || body.allowOlderCheckout === null ? false : body.allowOlderCheckout;
    if (typeof allowOlderCheckout !== "boolean") {
      fail("the allowOlderCheckout setting must be true or false");
    }

    // ---- Validate the request on its own -------------------------------
    // CSV files carry their header line and every row; the server checks
    // that together they are the file with the stated byte hash, so the run
    // records exactly what the CSV said.
    const CSV_ROLES = ["catalog-csv", "substitutes-csv"];
    const csvRows = (source, path, headerRaw) => {
      const rows = source.rows;
      if (!Array.isArray(rows) || rows.some((row) => typeof row !== "string" || row.length > DOCUMENT)) {
        fail(`the rows of ${path} must be a list of text`);
      }
      if (headerRaw === null || $security.sha256(headerRaw + rows.join("")) !== source.byteSha256) {
        fail(`the header line and rows of ${path} are not the file with its byte hash`);
      }
      return rows;
    };
    const sources = list("sources").map((source) => {
      const role = text(source, "role", 50, true);
      const path = text(source, "path", 500, true);
      const headerRaw = source.headerRaw === null || source.headerRaw === undefined ? null : text(source, "headerRaw", DOCUMENT, false);
      const isCsv = CSV_ROLES.indexOf(role) >= 0;
      if (!isCsv && source.rows !== undefined && source.rows !== null) {
        fail(`rows are kept only for CSV files, not for ${path}`);
      }
      return {
        role: role,
        path: path,
        byte_sha256: isSha(source.byteSha256) ? source.byteSha256 : fail("source byteSha256 is invalid"),
        size: Number.isInteger(source.size) && source.size >= 0 ? source.size : fail("source size is invalid"),
        header_raw: headerRaw,
        rows: isCsv ? csvRows(source, path, headerRaw) : null,
      };
    });
    if (sources.length === 0) {
      fail("sources must not be empty");
    }
    const onlySource = (role) => {
      const found = sources.filter((source) => source.role === role);
      if (found.length > 1) {
        fail(`sources name more than one ${role} file`);
      }
      return found.length === 1 ? found[0] : null;
    };
    const catalogCsv = onlySource("catalog-csv");
    const substitutesCsv = onlySource("substitutes-csv");
    if (catalogCsv && catalogCsv.path !== `database/${csvFile}.csv`) {
      fail(`the catalog-csv source of ${csvFile} must be database/${csvFile}.csv`);
    }

    const blobs = list("blobs").map((blob) => {
      if (!isSha(blob.sha256)) {
        fail("blob sha256 is invalid");
      }
      const bytes = lib.base64Decode(blob.base64);
      if (bytes === null) {
        fail(`blob ${blob.sha256} is not base64`);
      }
      if (bytes.length !== blob.size || lib.sha256Bytes(bytes) !== blob.sha256) {
        fail(`blob ${blob.sha256} does not match its bytes`);
      }
      if (bytes.length === 0) {
        // PocketBase cannot store an empty file.
        fail(`blob ${blob.sha256} is empty; blobs must have at least one byte`);
      }
      return { id: lib.deriveId("blob", blob.sha256), sha256: blob.sha256, bytes: bytes };
    });
    const requestBlobs = {};
    for (const blob of blobs) {
      if (requestBlobs[blob.sha256]) {
        fail(`blob ${blob.sha256} appears twice`);
      }
      requestBlobs[blob.sha256] = blob;
    }

    const KINDS = ["symbol", "footprint", "model-3d"];
    const cadItemRevisions = list("cadItemRevisions").map((revision) => {
      const kind = text(revision, "kind", 20, true);
      if (KINDS.indexOf(kind) < 0) {
        fail(`cad_item kind ${kind} is unknown`);
      }
      const key = text(revision, "key", 500, true);
      const cadItemId = lib.deriveId("cad-item", kind + "\n" + key);
      if (revision.cadItemId !== cadItemId) {
        fail(`cad_item ID for ${kind} ${key} must be ${cadItemId}`);
      }
      const snapshot = text(revision, "snapshot", DOCUMENT, true);
      const snapshotSha256 = $security.sha256(snapshot);
      if (revision.snapshotSha256 !== snapshotSha256) {
        fail(`snapshotSha256 of ${kind} ${key} does not match its snapshot`);
      }
      const id = lib.deriveId("cad-item-revision", cadItemId + "\n" + snapshotSha256);
      if (revision.id !== id) {
        fail(`CAD item revision ID for ${kind} ${key} must be ${id}`);
      }
      const parsed = parse(snapshot, "snapshot");
      if (parsed.kind !== kind || parsed.key !== key || !isSha(parsed.blob_sha256)) {
        fail(`snapshot of ${kind} ${key} disagrees with its revision`);
      }
      const license = parsed.license && typeof parsed.license.status === "string" ? parsed.license.status : "";
      if (license.length === 0) {
        fail(`snapshot of ${kind} ${key} has no license snapshot`);
      }
      return {
        id: id,
        cadItemId: cadItemId,
        kind: kind,
        key: key,
        snapshotSha256: snapshotSha256,
        snapshot: snapshot,
        parsed: parsed,
        license: license,
        source: text(revision, "source", DOCUMENT, true),
      };
    });
    const requestRevisions = {};
    for (const revision of cadItemRevisions) {
      if (requestRevisions[revision.id]) {
        fail(`CAD item revision ${revision.id} appears twice`);
      }
      requestRevisions[revision.id] = revision;
    }
    // A blob is only ever saved for a CAD item revision, so that every
    // write belongs to a Change.
    for (const blob of blobs) {
      if (!cadItemRevisions.some((revision) => revision.parsed.blob_sha256 === blob.sha256)) {
        fail(`blob ${blob.sha256} is not used by any CAD item revision in this request`);
      }
    }

    const parts = list("parts").map((part) => {
      const ipn = text(part, "ipn", 13, true);
      if (!/^[A-Z]{3}-[0-9]{4}-[0-9A-Z]{4}$/.test(ipn)) {
        fail(`${ipn} is not an IPN`);
      }
      const id = lib.deriveId("part", ipn);
      if (part.id !== id) {
        fail(`part ID for ${ipn} must be ${id}`);
      }
      const snapshot = text(part, "snapshot", DOCUMENT, true);
      const snapshotSha256 = $security.sha256(snapshot);
      if (part.snapshotSha256 !== snapshotSha256) {
        fail(`snapshotSha256 of ${ipn} does not match its snapshot`);
      }
      const parsed = parse(snapshot, "snapshot");
      if (parsed.ipn !== ipn) {
        fail(`the snapshot of ${ipn} names ${parsed.ipn}`);
      }
      const source = text(part, "source", DOCUMENT, true);
      const bound = [];
      for (const binding of parsed.cad_references || []) {
        if (binding.target && binding.target.kind === "cad-item-revision") {
          bound.push(lib.deriveId("cad-item-revision", binding.target.cad_item + "\n" + binding.target.snapshot_sha256));
        }
      }
      const parsedSource = parse(source, "source");
      if (parsedSource.source !== lib.CSV_CATALOG_SOURCE || typeof parsedSource.raw_record !== "string") {
        fail(`the source of ${ipn} is not a CSV catalog row`);
      }
      // The row a part comes from must be one of the rows the run records.
      const sameRow = (file, row) =>
        file !== null &&
        row.path === file.path &&
        row.byte_sha256 === file.byte_sha256 &&
        Number.isInteger(row.row_number) &&
        row.row_number >= 1 &&
        file.rows[row.row_number - 1] === row.raw_record;
      if (!sameRow(catalogCsv, parsedSource)) {
        fail(`the source of ${ipn} is not a row of the catalog-csv file this request sends`);
      }
      for (const row of Array.isArray(parsedSource.substitutes_csv_rows) ? parsedSource.substitutes_csv_rows : []) {
        if (!row || !sameRow(substitutesCsv, row)) {
          fail(`a substitutes.csv row in the source of ${ipn} is not a row of the substitutes-csv file this request sends`);
        }
      }
      const expectedRevision = part.expectedRevision;
      if (!Number.isInteger(expectedRevision) || expectedRevision < 0) {
        fail(`the expected revision of ${ipn} must be the number of its current revision, or 0 for a new part`);
      }
      return {
        id: id,
        ipn: ipn,
        expectedRevision: expectedRevision,
        snapshot: snapshot,
        snapshotSha256: snapshotSha256,
        source: source,
        parsedSource: parsedSource,
        bound: bound,
      };
    });
    const seenIpns = {};
    for (const part of parts) {
      if (seenIpns[part.ipn]) {
        fail(`${part.ipn} appears twice`);
      }
      seenIpns[part.ipn] = true;
    }

    const problem = (code, subject, path, location, message, detail) => ({
      id: lib.deriveId("import-problem", code + "\n" + subject + "\n" + location + "\n" + $security.sha256(detail)),
      code: code,
      subject: subject,
      path: path,
      location: location,
      message: message,
      detail: detail,
    });
    const problems = list("problems").map((item) => {
      const built = problem(
        text(item, "code", 100, true),
        text(item, "subject", 500, true),
        text(item, "path", 500, true),
        text(item, "location", 2000, true),
        text(item, "message", 10000, true),
        text(item, "detail", DOCUMENT, true),
      );
      if (item.id !== built.id) {
        fail(`problem ID for ${built.code} at ${built.location} must be ${built.id}`);
      }
      return built;
    });

    // The request's identity, built from validated values in a fixed order
    // (the parsed body is a Go map, whose key order varies).
    const requestHash = $security.sha256(
      JSON.stringify([
        reason,
        csvFile,
        sourceCommit,
        allowOlderCheckout,
        // A CSV file's rows are fixed by its byte hash, checked above.
        sources.map((source) => [source.role, source.path, source.byte_sha256, source.size, source.header_raw]),
        blobs.map((blob) => blob.sha256),
        cadItemRevisions.map((revision) => [revision.id, revision.source]),
        parts.map((part) => [part.id, part.expectedRevision, part.snapshotSha256, part.source]),
        problems.map((item) => [item.id, item.path, item.message]),
      ]),
    );

    let reply = null;
    $app.runInTransaction((txApp) => {
      const byId = (collection, id) => {
        try {
          return txApp.findRecordById(collection, id);
        } catch (_) {
          return null;
        }
      };
      const first = (collection, filter, params, sort) => {
        const found = txApp.findRecordsByFilter(collection, filter, sort || "", 1, 0, params);
        return found.length > 0 ? found[0] : null;
      };

      const prior = first("catalog_commands", "correlation_id = {:id}", { id: correlationId });
      if (prior) {
        if (prior.getString("actor") !== actor || prior.getString("request_hash") !== requestHash) {
          reply = {
            status: 409,
            body: {
              outcome: "correlation_reused",
              correlationId: correlationId,
              message: "this correlation ID was already used for a different command",
            },
          };
          return;
        }
        reply = { status: prior.getInt("status"), body: JSON.parse(prior.getString("result")), replayed: true };
        return;
      }

      // ---- Phase 0: refuse if someone changed a part since it was read --
      // As for edits: 409 conflict, nothing written, and not stored under
      // the correlation ID, so a retry is checked again.
      const conflicts = [];
      for (const part of parts) {
        const stored = byId("catalog_parts", part.id);
        const currentNumber = stored ? stored.getInt("current_revision_number") : 0;
        if (currentNumber !== part.expectedRevision) {
          const current = stored ? byId("catalog_part_revisions", stored.getString("current_revision")) : null;
          conflicts.push({
            ipn: part.ipn,
            expectedRevision: part.expectedRevision,
            currentRevision: current ? lib.describeRevision(txApp, current) : null,
          });
        }
      }
      if (conflicts.length > 0) {
        reply = {
          status: 409,
          body: {
            outcome: "conflict",
            correlationId: correlationId,
            conflicts: conflicts,
            message: `someone else changed ${conflicts.map((item) => item.ipn).join(", ")} after this import read the team catalog; nothing was imported, so read the team catalog again and import again`,
          },
        };
        return;
      }

      // The latest earlier import run of this CSV file, if any.
      const previousEntry = first(
        "catalog_audit_entries",
        "csv_file = {:csv_file} && csv_import_number > 0",
        { csv_file: csvFile },
        "-csv_import_number",
      );
      let previousRun = null;
      if (previousEntry) {
        try {
          previousRun = JSON.parse(previousEntry.getString("sources"));
        } catch (_) {
          previousRun = null;
        }
      }

      // ---- Phase 1: decide what is new, writing nothing ----------------
      // A stored record found by derived ID is reused only if its content
      // matches; anything else is an ID collision and nothing is written.
      const collisions = [];
      const reuse = (collection, id, matches, what) => {
        const stored = byId(collection, id);
        if (stored && !matches(stored)) {
          collisions.push(`${collection}/${id}: the stored record is not ${what}`);
        }
        return stored;
      };
      const newBlobs = blobs.filter(
        (blob) => !reuse("catalog_blobs", blob.id, (r) => r.getString("sha256") === blob.sha256, `blob ${blob.sha256}`),
      );
      const blobExists = (sha) => !!requestBlobs[sha] || !!byId("catalog_blobs", lib.deriveId("blob", sha));
      const revisionExists = (id) => !!requestRevisions[id] || !!byId("catalog_cad_item_revisions", id);

      const newCadItems = [];
      const newCadItemRevisions = [];
      const pendingNumbers = {};
      const seenCadItems = {};
      for (const revision of cadItemRevisions) {
        if (!blobExists(revision.parsed.blob_sha256)) {
          fail(`CAD item revision ${revision.id} names blob ${revision.parsed.blob_sha256}, which is neither stored nor sent`);
        }
        for (const model of revision.parsed.model_references || []) {
          if (model.target && model.target.kind === "cad-item-revision") {
            const target = lib.deriveId("cad-item-revision", model.target.cad_item + "\n" + model.target.snapshot_sha256);
            if (!revisionExists(target)) {
              fail(`${revision.key} binds 3D model revision ${target}, which is neither stored nor sent`);
            }
          }
        }
        const storedCadItem = reuse(
          "catalog_cad_items",
          revision.cadItemId,
          (r) => r.getString("kind") === revision.kind && r.getString("key") === revision.key,
          `${revision.kind} ${revision.key}`,
        );
        if (!seenCadItems[revision.cadItemId] && !storedCadItem) {
          newCadItems.push(revision);
        }
        seenCadItems[revision.cadItemId] = true;
        const storedRevision = reuse(
          "catalog_cad_item_revisions",
          revision.id,
          (r) => r.getString("cad_item") === revision.cadItemId && r.getString("snapshot_sha256") === revision.snapshotSha256,
          `revision ${revision.snapshotSha256} of ${revision.key}`,
        );
        if (storedRevision) {
          continue;
        }
        let latest = pendingNumbers[revision.cadItemId];
        if (!latest) {
          const stored = first("catalog_cad_item_revisions", "cad_item = {:cad_item}", { cad_item: revision.cadItemId }, "-number");
          latest = stored ? { number: stored.getInt("number"), id: stored.id } : { number: 0, id: "" };
        }
        revision.number = latest.number + 1;
        revision.predecessor = latest.id;
        pendingNumbers[revision.cadItemId] = { number: revision.number, id: revision.id };
        newCadItemRevisions.push(revision);
      }

      const newParts = [];
      const updatedParts = [];
      const serverProblems = [];
      let unchangedParts = 0;
      // The exact bytes of the substitutes.csv rows a source records.
      const substitutesCsvRows = (source) =>
        (Array.isArray(source.substitutes_csv_rows) ? source.substitutes_csv_rows : []).map((row) => row.raw_record);
      for (const part of parts) {
        for (const bound of part.bound) {
          if (!revisionExists(bound)) {
            fail(`${part.ipn} binds CAD item revision ${bound}, which is neither stored nor sent`);
          }
        }
        const stored = byId("catalog_parts", part.id);
        if (!stored) {
          part.revisionAfter = 1;
          newParts.push(part);
          continue;
        }
        if (stored.getString("ipn") !== part.ipn) {
          fail(`part ${part.id} has IPN ${stored.getString("ipn")}, not ${part.ipn}`);
        }
        const currentRevision = byId("catalog_part_revisions", stored.getString("current_revision"));
        if (!currentRevision) {
          fail(`part ${part.ipn} has no current revision`);
        }
        const location = `rows[IPN=${part.ipn}]`;
        part.revisionAfter = currentRevision.getInt("number");
        if (currentRevision.getString("snapshot_sha256") === part.snapshotSha256) {
          unchangedParts++;
          let previousSource = null;
          try {
            previousSource = JSON.parse(currentRevision.getString("source"));
          } catch (_) {
            previousSource = null;
          }
          const newSource = part.parsedSource;
          if (!previousSource || previousSource.source !== lib.CSV_CATALOG_SOURCE || typeof previousSource.raw_record !== "string") {
            serverProblems.push(
              problem(
                "current-revision-without-csv-source",
                part.ipn,
                newSource.path || "",
                location,
                `the current revision ${currentRevision.getInt("number")} of ${part.ipn} has no CSV source to compare the row with; the row is kept here and no revision is created`,
                JSON.stringify({
                  current_revision_number: currentRevision.getInt("number"),
                  path: newSource.path,
                  row_number: newSource.row_number,
                  raw_record: newSource.raw_record,
                  substitutes_csv_rows: substitutesCsvRows(newSource),
                }),
              ),
            );
          } else if (
            previousSource.raw_record !== newSource.raw_record ||
            JSON.stringify(substitutesCsvRows(previousSource)) !== JSON.stringify(substitutesCsvRows(newSource))
          ) {
            // The part's row or one of its substitutes.csv rows changed while
            // its part details did not, so only supplier data changed (such
            // as an LCSC code or a SupplierPN).
            serverProblems.push(
              problem(
                "csv-row-changed-without-part-details-change",
                part.ipn,
                newSource.path || "",
                location,
                `the CSV rows of ${part.ipn} changed but its part details did not (for example only its supplier data); the new rows are kept here and no revision is created`,
                // Only the rows identify it, so a later file version with
                // the same rows adds nothing.
                JSON.stringify({
                  current_revision_number: currentRevision.getInt("number"),
                  path: newSource.path,
                  row_number: newSource.row_number,
                  previous_raw_record: previousSource.raw_record,
                  raw_record: newSource.raw_record,
                  previous_substitutes_csv_rows: substitutesCsvRows(previousSource),
                  substitutes_csv_rows: substitutesCsvRows(newSource),
                }),
              ),
            );
          }
          continue;
        }
        const currentChange = byId("catalog_changes", currentRevision.getString("change"));
        if (currentChange && currentChange.getString("kind") === lib.CHANGE_KINDS.csvImport) {
          part.number = currentRevision.getInt("number") + 1;
          part.revisionAfter = part.number;
          part.predecessor = currentRevision.id;
          part.stored = stored;
          updatedParts.push(part);
        } else {
          serverProblems.push(
            problem(
              "csv-row-conflicts-with-current-revision",
              part.ipn,
              part.parsedSource.path || "",
              location,
              `the source row of ${part.ipn} changed, but its current revision ${currentRevision.getInt("number")} was not published by a CSV import; the row is kept here and the current revision is left as it is`,
              JSON.stringify({
                current_revision_number: currentRevision.getInt("number"),
                current_revision_change_kind: currentChange ? currentChange.getString("kind") : "",
                path: part.parsedSource.path,
                row_number: part.parsedSource.row_number,
                raw_record: part.parsedSource.raw_record,
                substitutes_csv_rows: substitutesCsvRows(part.parsedSource),
                snapshot: part.snapshot,
              }),
            ),
          );
        }
      }

      // A part the previous import run of this CSV file made, but no row
      // makes now, keeps its revisions; an import problem says it is gone.
      if (catalogCsv && previousRun && Array.isArray(previousRun.parts)) {
        for (const previous of previousRun.parts) {
          if (!previous || typeof previous.ipn !== "string" || seenIpns[previous.ipn]) {
            continue;
          }
          const stored = byId("catalog_parts", lib.deriveId("part", previous.ipn));
          serverProblems.push(
            problem(
              "part-not-in-csv-file",
              previous.ipn,
              catalogCsv.path,
              `rows[IPN=${previous.ipn}]`,
              `${previous.ipn} was a row of ${catalogCsv.path} at the previous import, but no row of it makes that part now; the part and its revisions are kept as they are`,
              JSON.stringify({
                path: catalogCsv.path,
                previous_import: previousEntry.id,
                current_revision_number: stored ? stored.getInt("current_revision_number") : 0,
              }),
            ),
          );
        }
      }

      const newProblems = [];
      const seenProblems = {};
      for (const item of problems.concat(serverProblems)) {
        const stored = reuse(
          "catalog_import_problems",
          item.id,
          (r) =>
            r.getString("code") === item.code &&
            r.getString("subject") === item.subject &&
            r.getString("location") === item.location &&
            $security.sha256(r.getString("detail")) === $security.sha256(item.detail),
          `problem ${item.code} at ${item.location}`,
        );
        if (!seenProblems[item.id] && !stored) {
          newProblems.push(item);
        }
        seenProblems[item.id] = true;
      }

      if (collisions.length > 0) {
        // Not stored under the correlation ID: nothing was decided.
        reply = {
          status: 409,
          body: {
            outcome: "id_collision",
            correlationId: correlationId,
            collisions: collisions,
            message: "stored records have the derived IDs of this request but other content",
          },
        };
        return;
      }

      const unchanged = {
        blobs: blobs.length - newBlobs.length,
        cadItems: Object.keys(seenCadItems).length - newCadItems.length,
        cadItemRevisions: cadItemRevisions.length - newCadItemRevisions.length,
        parts: unchangedParts,
        problems: Object.keys(seenProblems).length - newProblems.length,
      };
      const created = {
        blobs: newBlobs.length,
        cadItems: newCadItems.length,
        cadItemRevisions: newCadItemRevisions.length,
        parts: newParts.length,
        partRevisions: newParts.length + updatedParts.length,
        updatedParts: updatedParts.length,
        problems: newProblems.length,
      };
      // New blobs and CAD items only ever arrive with a new CAD item revision.
      const revisionCount = created.partRevisions + created.cadItemRevisions;

      // Whether the previous import run of this CSV file read exactly these
      // files. If not, this run is recorded even when it adds nothing else,
      // so the latest run always holds what the CSV says now.
      const fileList = (files) =>
        JSON.stringify(
          (Array.isArray(files) ? files : [])
            .map((file) => [file.path, file.byte_sha256])
            .sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)),
        );
      const sameFilesAsPreviousRun = previousRun !== null && fileList(previousRun.files) === fileList(sources);

      let status;
      let result;
      if (revisionCount === 0 && newProblems.length === 0 && sameFilesAsPreviousRun) {
        status = 422;
        result = { outcome: "noop", correlationId: correlationId, unchanged: unchanged };
      } else {
        // ---- Phase 2: save the run ------------------------------------
        const collection = (name) => txApp.findCollectionByNameOrId(name);
        const record = (name, id) => {
          const made = new Record(collection(name));
          made.set("id", id);
          return made;
        };

        // A Change only when there is a revision to put in it.
        let changeId = "";
        let seq = 0;
        if (revisionCount > 0) {
          const last = first("catalog_changes", "seq > 0", {}, "-seq");
          seq = last ? last.getInt("seq") + 1 : 1;
          changeId = lib.deriveId("change", correlationId);
          const change = record("catalog_changes", changeId);
          change.set("seq", seq);
          change.set("kind", lib.CHANGE_KINDS.csvImport);
          change.set("reason", reason);
          change.set("correlation_id", correlationId);
          change.set("actor", actor);
          change.set("actor_name", actorName);
          change.set("source_commit", sourceCommit);
          change.set(
            "sources",
            JSON.stringify({
              csv_file: csvFile,
              files: sources.map((source) => ({
                role: source.role,
                path: source.path,
                byte_sha256: source.byte_sha256,
                size: source.size,
                header_raw: source.header_raw,
              })),
            }),
          );
          change.set("summary", JSON.stringify(created));
          txApp.save(change);
        }

        // Every run that writes anything has an audit entry.
        const auditEntryId = lib.deriveId("audit-entry", correlationId);
        const audit = record("catalog_audit_entries", auditEntryId);
        audit.set("action", "catalog.csv-import");
        audit.set("actor", actor);
        audit.set("actor_name", actorName);
        audit.set("change", changeId);
        audit.set("reason", reason);
        audit.set("correlation_id", correlationId);
        audit.set("summary", JSON.stringify(created));
        const lastRun = first("catalog_audit_entries", "csv_import_number > 0", {}, "-csv_import_number");
        audit.set("csv_file", csvFile);
        audit.set("csv_import_number", lastRun ? lastRun.getInt("csv_import_number") + 1 : 1);
        audit.set("source_commit", sourceCommit);
        audit.set("allowed_older_checkout", allowOlderCheckout);
        audit.set(
          "sources",
          JSON.stringify({
            csv_file: csvFile,
            files: sources.map((source) => {
              const file = {
                role: source.role,
                path: source.path,
                byte_sha256: source.byte_sha256,
                size: source.size,
                header_raw: source.header_raw,
              };
              if (source.rows !== null) {
                file.rows = source.rows;
              }
              return file;
            }),
            parts: parts.map((part) => ({ ipn: part.ipn, revision: part.revisionAfter })),
          }),
        );
        txApp.save(audit);

        for (const blob of newBlobs) {
          const made = record("catalog_blobs", blob.id);
          made.set("sha256", blob.sha256);
          made.set("size", blob.bytes.length);
          made.set("file", $filesystem.fileFromBytes(blob.bytes, blob.sha256 + ".bin"));
          made.set("first_change", changeId);
          txApp.save(made);
        }
        for (const cadItem of newCadItems) {
          const made = record("catalog_cad_items", cadItem.cadItemId);
          made.set("kind", cadItem.kind);
          made.set("key", cadItem.key);
          made.set("first_change", changeId);
          txApp.save(made);
        }
        for (const revision of newCadItemRevisions) {
          const made = record("catalog_cad_item_revisions", revision.id);
          made.set("cad_item", revision.cadItemId);
          made.set("number", revision.number);
          made.set("predecessor", revision.predecessor);
          made.set("change", changeId);
          made.set("blob", lib.deriveId("blob", revision.parsed.blob_sha256));
          made.set("snapshot_sha256", revision.snapshotSha256);
          made.set("snapshot", revision.snapshot);
          made.set("license", revision.license);
          made.set("source", revision.source);
          txApp.save(made);
        }
        const savePartRevision = (part, number, predecessor) => {
          const revisionId = lib.deriveId("part-revision", part.id + "\n" + number);
          const made = record("catalog_part_revisions", revisionId);
          made.set("part", part.id);
          made.set("number", number);
          made.set("predecessor", predecessor);
          made.set("change", changeId);
          made.set("snapshot", part.snapshot);
          made.set("snapshot_sha256", part.snapshotSha256);
          made.set("source", part.source);
          made.set("cad_item_revisions", part.bound);
          txApp.save(made);
          return revisionId;
        };
        for (const part of newParts) {
          const made = record("catalog_parts", part.id);
          made.set("ipn", part.ipn);
          made.set("ipn_prefix", part.ipn.substring(0, 3));
          made.set("variant", part.ipn.substring(9));
          made.set("current_revision_number", 1);
          made.set("first_change", changeId);
          txApp.save(made);
          made.set("current_revision", savePartRevision(part, 1, ""));
          txApp.save(made);
        }
        for (const part of updatedParts) {
          part.stored.set("current_revision", savePartRevision(part, part.number, part.predecessor));
          part.stored.set("current_revision_number", part.number);
          txApp.save(part.stored);
        }
        for (const item of newProblems) {
          const made = record("catalog_import_problems", item.id);
          made.set("code", item.code);
          made.set("subject", item.subject);
          made.set("path", item.path);
          made.set("location", item.location);
          made.set("message", item.message);
          made.set("detail", item.detail);
          made.set("audit_entry", auditEntryId);
          txApp.save(made);
        }

        status = 200;
        result =
          revisionCount > 0
            ? {
                outcome: "applied",
                correlationId: correlationId,
                changeId: changeId,
                seq: seq,
                auditEntryId: auditEntryId,
                created: created,
                unchanged: unchanged,
              }
            : {
                outcome: "nothing_changed",
                correlationId: correlationId,
                auditEntryId: auditEntryId,
                problemsRecorded: newProblems.length,
                unchanged: unchanged,
              };
      }

      const ledger = new Record(txApp.findCollectionByNameOrId("catalog_commands"));
      ledger.set("correlation_id", correlationId);
      ledger.set("actor", actor);
      ledger.set("request_hash", requestHash);
      ledger.set("status", status);
      ledger.set("result", JSON.stringify(result));
      txApp.save(ledger);
      reply = { status: status, body: result };
    });

    if (reply.replayed) {
      e.response.header().set("X-Collab-Replayed", "true");
    }
    return e.json(reply.status, reply.body);
  },
  $apis.requireSuperuserAuth(),
);

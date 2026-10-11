/// <reference path="../pb_data/types.d.ts" />

// POST /api/catalog/csv-imports: saves one CSV catalog import. Superusers
// only. The request is built by crates/catalog (CsvImport and
// pocketbase::ImportCommand):
//
//   { correlationId, reason, csvFile, sourceCommit, allowOlderCheckout,
//     allowUncommittedChanges, expectedCsvImport,
//     sources: [{ role, path, byteSha256, size, headerRaw, rows }],
//     blobs: [{ sha256, size, text | base64 }],
//     cadItemRevisions: [{ id, cadItemId, kind, key, expectedRevision,
//                          number, snapshotSha256, snapshot, source }],
//     parts: [{ id, ipn, expectedRevision, expectedDetails,
//               details: { snapshot, snapshotSha256 },
//               revision: { snapshot, snapshotSha256 }, source }],
//     problems: [{ id, code, subject, path, location, message, detail }] }
//
// Every run sends exactly one catalog-csv source (database/<csvFile>.csv)
// and one substitutes-csv source (database/substitutes.csv). `rows` is
// given for these two only: every row's exact bytes, in file order, none
// empty. `expectedCsvImport` is the number of the latest import run of this
// CSV file that the importer checked its checkout against, or 0.
// `expectedRevision` and `expectedDetails` are the numbers of the part's
// current revision and current details when the importer read the team
// catalog, or 0 for a part it expects to be new. A part's `details` are its
// part details (describing fields and manufacturer parts) and its
// `revision` is the CAD it uses (Symbol and Footprint fields, bindings and
// Excluded from BOM); `source` is the CSV row both came from. `allowOlderCheckout` says
// the importer was told to import from a checkout older than the team
// catalog's latest imports (the importer checks git ancestry; the server
// cannot), and is recorded on the audit entry. A `sourceCommit` ending in
// `+dirty` means the files have changes that are not committed; such an
// import is refused (400) unless `allowUncommittedChanges` is true, which
// says the importer was told to import them anyway, and is recorded on the
// audit entry too. The server cannot run git, so it trusts the importer to
// mark uncommitted changes; it checks only that the mark comes with the
// setting.
//
// CAD item revisions are numbered from 1 per CAD item, like part
// revisions, and the current revision is the newest. A request sends one
// revision per CAD item. Its `expectedRevision` is the number of the CAD
// item's current revision when the importer read the team catalog (0 for a
// new CAD item), and its `number` is that same number if the content is the
// current revision's, otherwise the next one, even if an older revision
// had exactly this content. Part details and footprint snapshots name each
// CAD item revision by CAD item, number and snapshot hash. A binding keeps
// the number it has now while its content is the same (the same content
// under a newer number is not a change); any other binding must name the
// CAD item's current revision as it is after this request.
//
// The server trusts none of the derived values: it re-hashes every blob and
// snapshot, re-derives every record ID, checks that each CSV file's header
// line and rows are the file with its byte hash and that every part's
// source row is one of those rows, checks every CAD item revision number
// against what is stored, and checks that every CAD reference names the
// current revision of its CAD item, or the revision with the same content
// that it binds now. Each part revision's Excluded from BOM must be what the
// bound CAD's own flags say in the bytes the server has (catalog_lib.js
// excludedFromBom, kicad-parts-ccc0); otherwise 400.
//
// Then, in one transaction, it compares the request with what is stored:
// - if the latest import run of this CSV file is not the one the importer
//   checked against, another import landed in between: 409
//   newer_import_run and nothing is written;
// - if any part's current revision or details, or any CAD item's current
//   revision, is not the one the importer expected, someone changed it in
//   between: 409 conflict and nothing is written;
// - blobs, CAD items and import problems that already exist are reused
//   (they are content-addressed), after checking that the stored record's
//   hash or key really matches, so an ID collision is refused, not merged;
// - a CAD item revision whose content is the current revision's is that
//   revision; any other content is a new revision numbered after it;
// - a new IPN becomes a part with details 1 and revision 1;
// - an existing part's details and CAD are compared separately. Details
//   that differ are saved as the part's next details only if its current
//   details came from a CSV import; CAD that differs is saved as its next
//   revision only if its current revision came from a CSV import.
//   Otherwise an import problem keeps the row, so a re-import never
//   overwrites an edit made in the Catalog, while an edit of one kind does
//   not stop the CSV from changing the other. A new revision comes after
//   the part's newest revision, which may be archived;
// - an import never brings an archived design back: CAD whose content
//   matches an archived revision of the part adds nothing, and an import
//   problem (csv-cad-matches-archived-revision) keeps the row, on every run;
// - a part whose details and CAD are both unchanged stays as it is. If its
//   source row changed anyway (for example its supplier data), or the part
//   was last saved by an edit, so there is no CSV row to compare with, an
//   import problem keeps the new row;
// - a part that an earlier import run of this CSV file made, but no row
//   makes now, gets an import problem, found again by every later run;
//   nothing is deleted.
//
// A Change is created only when there is at least one new revision or
// saved part details. Every
// run that writes anything gets an audit entry, and new import problems
// point to it. A CSV import run's audit entry also records exactly what the
// CSV said (the files read, their byte hashes, each CSV file's header line
// and rows), the current revision and details it left each part at, and
// the IDs of
// every import problem it found, new or already stored. A run is
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
//         expectedDetails, currentRevision, currentDetails }],
//         cadItemConflicts: [{ kind, key, expectedRevision,
//         currentRevision }], message }
//                                          someone changed these parts or
//                                          CAD items after the importer read
//                                          them; nothing is written
//   409 { outcome: "newer_import_run", csvFile, expectedCsvImport,
//         latestCsvImport, message }   another import of this CSV file
//                                          landed first; nothing is written
//   409 { outcome: "correlation_reused" } the ID was used for another request
//   409 { outcome: "id_collision", collisions } a stored record has a derived
//                                          ID of this request but other
//                                          content; nothing is written
//   400 bad or inconsistent input, 401/403 not a superuser.
// Answers with status 200 or 422 are stored under the correlation ID and
// replayed byte for byte (with X-Collab-Replayed: true) without writing
// anything. Conflicts and newer_import_run are not stored, so a retry is
// checked again.
//
// A request may be at most 32 MiB (IMPORT_BODY_LIMIT in
// crates/catalog/src/pocketbase.rs, which refuses a bigger one before
// sending it); PocketBase answers 413 to anything larger. The largest
// import of the CSV catalog today (g-con) is 1.8 MiB (1,900,969 bytes).
const IMPORT_BODY_LIMIT = 32 * 1024 * 1024;
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
    const allowUncommittedChanges =
      body.allowUncommittedChanges === undefined || body.allowUncommittedChanges === null ? false : body.allowUncommittedChanges;
    if (typeof allowUncommittedChanges !== "boolean") {
      fail("the allowUncommittedChanges setting must be true or false");
    }
    if (sourceCommit.endsWith("+dirty") && !allowUncommittedChanges) {
      fail(
        "the files of this import have changes that are not committed (its source commit ends in +dirty). " +
          "Commit them and import again, or allow uncommitted changes to import them anyway",
      );
    }
    // The number of the latest import run of this CSV file that the
    // importer checked its checkout against (0 if there was none).
    const expectedCsvImport = body.expectedCsvImport;
    if (!Number.isInteger(expectedCsvImport) || expectedCsvImport < 0) {
      fail("the expected CSV import number must be the number of the latest import run of this CSV file, or 0");
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
      // Every CSV row has at least its line ending, so an empty row could
      // only pad the list without changing the file's hash.
      if (rows.some((row) => row.length === 0)) {
        fail(`the rows of ${path} include an empty row; each row must be one whole row of the file`);
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
    // Every run reads exactly one catalog-csv file and one substitutes.csv,
    // so the latest run of a CSV file always holds that whole file.
    const onlySource = (role, path) => {
      const found = sources.filter((source) => source.role === role);
      if (found.length !== 1) {
        fail(`sources must name exactly one ${role} file, not ${found.length}`);
      }
      if (found[0].path !== path) {
        fail(`the ${role} source of ${csvFile} must be ${path}`);
      }
      return found[0];
    };
    const catalogCsv = onlySource("catalog-csv", `database/${csvFile}.csv`);
    const substitutesCsv = onlySource("substitutes-csv", "database/substitutes.csv");

    // A blob comes as `text` when its bytes are UTF-8 (every real CAD file
    // so far) and as `base64` otherwise. Text is hashed and stored by
    // PocketBase's own Go code: $security.sha256(text) and toBytes(text)
    // both turn the same text into the same UTF-8 bytes, so the stored
    // bytes are exactly the hashed ones, at about 50 ms per MiB. Base64 is
    // decoded and hashed here in JavaScript (about 3 s per MiB), because
    // PocketBase has no way to hash bytes that are not text.
    const blobs = list("blobs").map((blob) => {
      if (!isSha(blob.sha256)) {
        fail("blob sha256 is invalid");
      }
      const hasText = blob.text !== undefined && blob.text !== null;
      const hasBase64 = blob.base64 !== undefined && blob.base64 !== null;
      if (hasText === hasBase64) {
        fail(`blob ${blob.sha256} must have either text or base64`);
      }
      let bytes;
      let sha256;
      if (hasText) {
        if (typeof blob.text !== "string") {
          fail(`the text of blob ${blob.sha256} must be a string`);
        }
        bytes = toBytes(blob.text);
        sha256 = $security.sha256(blob.text);
      } else {
        bytes = lib.base64Decode(blob.base64);
        if (bytes === null) {
          fail(`blob ${blob.sha256} is not base64`);
        }
        sha256 = lib.sha256Bytes(bytes);
      }
      if (bytes.length !== blob.size || sha256 !== blob.sha256) {
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
      const expectedRevision = revision.expectedRevision;
      if (!Number.isInteger(expectedRevision) || expectedRevision < 0) {
        fail(`the expected revision of ${kind} ${key} must be the number of its current revision, or 0 for a new CAD item`);
      }
      // The current revision's number for its own content, or the next one
      // for new content. Which of the two is right is checked against what
      // is stored, in the transaction.
      const number = revision.number;
      if (!Number.isInteger(number) || (number !== expectedRevision && number !== expectedRevision + 1)) {
        fail(`the revision number of ${kind} ${key} must be ${expectedRevision} or ${expectedRevision + 1}`);
      }
      const id = lib.deriveId("cad-item-revision", cadItemId + "\n" + number);
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
      const models = [];
      for (const model of parsed.model_references || []) {
        if (model && model.target && model.target.kind === "cad-item-revision") {
          models.push(lib.cadItemRevisionTarget(model.target, `the 3D model ${model.reference} of ${key}`));
        }
      }
      return {
        id: id,
        cadItemId: cadItemId,
        kind: kind,
        key: key,
        expectedRevision: expectedRevision,
        number: number,
        snapshotSha256: snapshotSha256,
        snapshot: snapshot,
        parsed: parsed,
        license: license,
        models: models,
        source: text(revision, "source", DOCUMENT, true),
      };
    });
    const requestRevisions = {};
    for (const revision of cadItemRevisions) {
      if (requestRevisions[revision.cadItemId]) {
        fail(`${revision.kind} ${revision.key} appears twice; send one revision per CAD item`);
      }
      requestRevisions[revision.cadItemId] = revision;
    }
    // A blob is only ever saved for a CAD item revision, so that every
    // write belongs to a Change.
    for (const blob of blobs) {
      if (!cadItemRevisions.some((revision) => revision.parsed.blob_sha256 === blob.sha256)) {
        fail(`blob ${blob.sha256} is not used by any CAD item revision in this request`);
      }
    }

    // A part's details or revision document: its exact text, hashed here,
    // checked for its schema, its IPN and what it may hold (catalog_lib.js
    // detailsProblem and revisionProblem: Symbol and Footprint only in the
    // revision, matching its CAD references; every other field only in the
    // details).
    const document = (part, name, schema, ipn) => {
      const entry = part[name];
      if (!entry || typeof entry !== "object") {
        fail(`${ipn} needs ${name}`);
      }
      const snapshot = text(entry, "snapshot", DOCUMENT, true);
      const snapshotSha256 = $security.sha256(snapshot);
      if (entry.snapshotSha256 !== snapshotSha256) {
        fail(`the snapshotSha256 of ${ipn}'s ${name} does not match its snapshot`);
      }
      const parsed = parse(snapshot, `${ipn}'s ${name}`);
      if (parsed.schema !== schema || parsed.ipn !== ipn) {
        fail(`the ${name} of ${ipn} must be ${schema} for ${ipn}`);
      }
      const problem = name === "details" ? lib.detailsProblem(parsed) : lib.revisionProblem(parsed);
      if (problem) {
        fail(`the ${name} of ${ipn} cannot be saved: ${problem}`);
      }
      return { snapshot: snapshot, sha: snapshotSha256, parsed: parsed };
    };
    const parts = list("parts").map((part) => {
      const ipn = text(part, "ipn", 13, true);
      if (!/^[A-Z]{3}-[0-9]{4}-[0-9A-Z]{4}$/.test(ipn)) {
        fail(`${ipn} is not an IPN`);
      }
      const id = lib.deriveId("part", ipn);
      if (part.id !== id) {
        fail(`part ID for ${ipn} must be ${id}`);
      }
      const details = document(part, "details", lib.PART_DETAILS_SCHEMA, ipn);
      const revision = document(part, "revision", lib.PART_REVISION_SCHEMA, ipn);
      const source = text(part, "source", DOCUMENT, true);
      const targets = [];
      for (const binding of revision.parsed.cad_references || []) {
        if (binding && binding.target && binding.target.kind === "cad-item-revision") {
          const target = lib.cadItemRevisionTarget(binding.target, `${ipn}'s ${binding.reference}`);
          // A symbol or footprint reference binds only its own CAD item, as
          // edits do, so no reference can point at other bytes.
          if (
            (binding.role !== "symbol" && binding.role !== "footprint") ||
            typeof binding.reference !== "string" ||
            target.cadItem !== lib.deriveId("cad-item", binding.role + "\n" + binding.reference)
          ) {
            fail(`${ipn}'s ${binding.role} ${binding.reference} must bind the CAD item of that ${binding.role}`);
          }
          targets.push(target);
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
      const expected = (name) => {
        const value = part[name];
        if (!Number.isInteger(value) || value < 0) {
          fail(`the ${name === "expectedRevision" ? "expected revision" : "expected details"} of ${ipn} must be the number of its current one, or 0 for a new part`);
        }
        return value;
      };
      return {
        id: id,
        ipn: ipn,
        expectedRevision: expected("expectedRevision"),
        expectedDetails: expected("expectedDetails"),
        details: details,
        revision: revision,
        source: source,
        parsedSource: parsedSource,
        targets: targets,
        bound: targets.map((target) => target.id),
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
        allowUncommittedChanges,
        expectedCsvImport,
        // A CSV file's rows are fixed by its byte hash, checked above.
        sources.map((source) => [source.role, source.path, source.byte_sha256, source.size, source.header_raw]),
        blobs.map((blob) => blob.sha256),
        cadItemRevisions.map((revision) => [revision.id, revision.expectedRevision, revision.snapshotSha256, revision.source]),
        parts.map((part) => [part.id, part.expectedRevision, part.expectedDetails, part.details.sha, part.revision.sha, part.source]),
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

      // The importer checked its checkout against the latest run it read.
      // If another import of this CSV file landed since, that check is
      // stale: 409, nothing written, not stored.
      const latestCsvImport = previousEntry ? previousEntry.getInt("csv_import_number") : 0;
      if (latestCsvImport !== expectedCsvImport) {
        reply = {
          status: 409,
          body: {
            outcome: "newer_import_run",
            correlationId: correlationId,
            csvFile: csvFile,
            expectedCsvImport: expectedCsvImport,
            latestCsvImport: latestCsvImport,
            message: `another import of ${catalogCsv.path} landed after this import read the team catalog: its latest import run is now number ${latestCsvImport} (counting the runs of every CSV file), not number ${expectedCsvImport}. Nothing was imported, so read the team catalog again and import again`,
          },
        };
        return;
      }

      // ---- Phase 0: refuse if someone changed a part since it was read --
      // As for edits: 409 conflict, nothing written, and not stored under
      // the correlation ID, so a retry is checked again.
      // At most 10 names, then how many more.
      const listed = (names) =>
        names.length <= 10 ? names.join(", ") : `${names.slice(0, 10).join(", ")} and ${names.length - 10} more`;
      const conflicts = [];
      const cadItemConflicts = [];
      // Each CAD item's newest stored revision, which is its current one.
      const storedCurrent = {};
      const currentStored = (cadItemId) => {
        if (!(cadItemId in storedCurrent)) {
          storedCurrent[cadItemId] = first("catalog_cad_item_revisions", "cad_item = {:cad_item}", { cad_item: cadItemId }, "-number");
        }
        return storedCurrent[cadItemId];
      };
      for (const revision of cadItemRevisions) {
        const current = currentStored(revision.cadItemId);
        const currentNumber = current ? current.getInt("number") : 0;
        if (currentNumber !== revision.expectedRevision) {
          cadItemConflicts.push({
            kind: revision.kind,
            key: revision.key,
            expectedRevision: revision.expectedRevision,
            currentRevision: currentNumber,
          });
        }
      }
      for (const part of parts) {
        const stored = byId("catalog_parts", part.id);
        const currentRevisionNumber = stored ? stored.getInt("current_revision_number") : 0;
        const currentDetailsNumber = stored ? stored.getInt("current_details_number") : 0;
        if (currentRevisionNumber !== part.expectedRevision || currentDetailsNumber !== part.expectedDetails) {
          const revision = stored ? byId("catalog_part_revisions", stored.getString("current_revision")) : null;
          const details = stored ? byId("catalog_part_details", stored.getString("current_details")) : null;
          conflicts.push({
            ipn: part.ipn,
            expectedRevision: part.expectedRevision,
            expectedDetails: part.expectedDetails,
            currentRevision: revision ? lib.describeRecord(txApp, revision) : null,
            currentDetails: details ? lib.describeRecord(txApp, details) : null,
          });
        }
      }
      if (conflicts.length > 0 || cadItemConflicts.length > 0) {
        const names = conflicts.map((item) => item.ipn).concat(cadItemConflicts.map((item) => item.key));
        reply = {
          status: 409,
          body: {
            outcome: "conflict",
            correlationId: correlationId,
            conflicts: conflicts,
            cadItemConflicts: cadItemConflicts,
            message: `someone else changed ${listed(names)} after this import read the team catalog; nothing was imported, so read the team catalog again and import again`,
          },
        };
        return;
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

      // A hash of a CAD item revision's content: its snapshot with the
      // numbers of its 3D model bindings left out. Revisions with the same
      // content differ at most in which numbers they name. Keys are sorted
      // first, so the order a client wrote them in cannot hide equal
      // content. crates/catalog CadItemDetails::content_sha256 makes the
      // same comparison in Rust but hashes in its own key order, so the two
      // hashes differ and must never be compared with each other.
      const contentOf = (snapshot) => lib.cadContent(snapshot);
      // Each CAD item's current revision as it is after this request.
      const currentAfter = {};
      const newCadItems = [];
      const newCadItemRevisions = [];
      for (const revision of cadItemRevisions) {
        if (!blobExists(revision.parsed.blob_sha256)) {
          fail(`the ${revision.kind} ${revision.key} names blob ${revision.parsed.blob_sha256}, which is neither stored nor sent`);
        }
        const storedCadItem = reuse(
          "catalog_cad_items",
          revision.cadItemId,
          (r) => r.getString("kind") === revision.kind && r.getString("key") === revision.key,
          `${revision.kind} ${revision.key}`,
        );
        if (!storedCadItem) {
          newCadItems.push(revision);
        }
        const current = currentStored(revision.cadItemId);
        const currentNumber = current ? current.getInt("number") : 0;
        const consistent = !current || lib.isCadItemRevision(current, revision.cadItemId, currentNumber);
        if (!consistent) {
          collisions.push(`catalog_cad_item_revisions/${current.id}: the stored record is not revision ${currentNumber} of ${revision.key}`);
        }
        if (current && current.getString("snapshot_sha256") === revision.snapshotSha256) {
          if (revision.number !== currentNumber) {
            fail(`the ${revision.kind} ${revision.key} has the content of its current revision ${currentNumber}, so its revision number must be ${currentNumber}`);
          }
        } else {
          // The same content under other numbers is not a change, so it
          // cannot be a new revision.
          if (current && consistent && contentOf(current.getString("snapshot")) === contentOf(revision.snapshot)) {
            fail(
              `the ${revision.kind} ${revision.key} has the same content as its current revision ${currentNumber} and differs only in the revision numbers it names; send revision ${currentNumber} as it is`,
            );
          }
          if (revision.number !== currentNumber + 1) {
            fail(`the ${revision.kind} ${revision.key} differs from its current revision ${currentNumber}, so its revision number must be ${currentNumber + 1}`);
          }
          // Nothing can be stored yet under the number after the current one.
          reuse("catalog_cad_item_revisions", revision.id, () => false, `a new revision ${revision.number} of ${revision.key}`);
          revision.predecessor = current ? current.id : "";
          newCadItemRevisions.push(revision);
        }
        currentAfter[revision.cadItemId] = {
          number: revision.number,
          snapshotSha256: revision.snapshotSha256,
          content: contentOf(revision.snapshot),
        };
      }
      // Footprints and parts must bind the current revision of each CAD
      // item, so an import never binds an older revision.
      const currentOf = (cadItemId) => {
        if (currentAfter[cadItemId]) {
          return currentAfter[cadItemId];
        }
        const stored = currentStored(cadItemId);
        if (!stored) {
          return null;
        }
        const number = stored.getInt("number");
        if (!lib.isCadItemRevision(stored, cadItemId, number)) {
          collisions.push(`catalog_cad_item_revisions/${stored.id}: the stored record is not revision ${number} of CAD item ${cadItemId}`);
        }
        currentAfter[cadItemId] = {
          number: number,
          snapshotSha256: stored.getString("snapshot_sha256"),
          content: contentOf(stored.getString("snapshot")),
        };
        return currentAfter[cadItemId];
      };
      // The CAD item revisions a stored snapshot binds, by CAD item, from
      // its part details' references or its footprint's 3D models.
      const boundBy = (snapshot) => {
        const bound = {};
        if (!snapshot) {
          return bound;
        }
        const parsed = JSON.parse(snapshot);
        const references = (parsed.cad_references || []).concat(parsed.model_references || []);
        for (const reference of references) {
          const target = reference && reference.target;
          if (target && target.kind === "cad-item-revision") {
            bound[target.cad_item] = { number: target.number, snapshotSha256: target.snapshot_sha256 };
          }
        }
        return bound;
      };
      // A binding must keep exactly the revision it binds now (`kept`, from
      // the stored current revision of the part or footprint) while that
      // has the same content as the CAD item's current revision after this
      // request: the same content under another number is not a change.
      // Any other binding must name that current revision.
      const checkBinding = (target, kept) => {
        const current = currentOf(target.cadItem);
        if (!current) {
          fail(`${target.who} names CAD item ${target.cadItem}, which is neither stored nor sent`);
        }
        // A stored record that is not what its ID says is answered as an ID
        // collision, below; its bindings cannot be checked against it.
        if (collisions.length > 0) {
          return;
        }
        // What the stored part or footprint binds now, if that revision has
        // the same content as the current one: then the binding must keep it.
        const now = kept[target.cadItem];
        const nowRecord = now ? byId("catalog_cad_item_revisions", lib.deriveId("cad-item-revision", target.cadItem + "\n" + now.number)) : null;
        const nowHasCurrentContent =
          !!nowRecord &&
          lib.isCadItemRevision(nowRecord, target.cadItem, now.number) &&
          nowRecord.getString("snapshot_sha256") === now.snapshotSha256 &&
          contentOf(nowRecord.getString("snapshot")) === current.content;
        if (nowHasCurrentContent) {
          if (target.number !== now.number || target.snapshotSha256 !== now.snapshotSha256) {
            fail(
              `${target.who} must keep revision ${now.number} of its CAD item, which it binds now: the current revision, ${current.number}, has the same content, and the same content under another number is not a change`,
            );
          }
          return;
        }
        if (current.number !== target.number || current.snapshotSha256 !== target.snapshotSha256) {
          fail(
            `${target.who} names revision ${target.number} of its CAD item, but its content changed, so an import must name the current revision, ${current.number}`,
          );
        }
      };
      for (const revision of cadItemRevisions) {
        const stored = currentStored(revision.cadItemId);
        const kept = boundBy(stored ? stored.getString("snapshot") : "");
        for (const model of revision.models) {
          checkBinding(model, kept);
        }
      }

      const newParts = [];
      const updatedParts = [];
      const serverProblems = [];
      let unchangedParts = 0;
      // The exact bytes of the substitutes.csv rows a source records.
      const substitutesCsvRows = (source) =>
        (Array.isArray(source.substitutes_csv_rows) ? source.substitutes_csv_rows : []).map((row) => row.raw_record);
      // The Change that saved a record: its kind and seq.
      const savedBy = (record) => {
        const change = byId("catalog_changes", record.getString("change"));
        return { kind: change ? change.getString("kind") : "", seq: change ? change.getInt("seq") : 0 };
      };
      // Whether a symbol or footprint CAD item revision a part binds says
      // it is not in the BOM: from this request's bytes when it sends that
      // revision, otherwise as stored. Each blob is judged at most once.
      const storedMarks = lib.storedBomMarks(txApp);
      const sentMarks = {};
      const markOf = (target, role) => {
        const sent = requestRevisions[target.cad_item];
        if (!sent || sent.number !== target.number) {
          return storedMarks(target, role);
        }
        if (sent.kind !== role) {
          return null;
        }
        const sha = sent.parsed.blob_sha256;
        const key = role + "\n" + sha;
        if (sentMarks[key] === undefined) {
          const blob = requestBlobs[sha];
          const stored = blob ? null : byId("catalog_blobs", lib.deriveId("blob", sha));
          sentMarks[key] = blob
            ? lib.bomMark(role, blob.bytes)
            : stored
              ? lib.bomMark(role, lib.blobBytes(stored))
              : null;
        }
        return sentMarks[key];
      };
      for (const part of parts) {
        const stored = byId("catalog_parts", part.id);
        const currentRevision = stored ? byId("catalog_part_revisions", stored.getString("current_revision")) : null;
        const kept = boundBy(currentRevision ? currentRevision.getString("snapshot") : "");
        for (const target of part.targets) {
          checkBinding(target, kept);
        }
        // Excluded from BOM must be what the bound CAD's own flags say, as
        // the server reads them (kicad-parts-ccc0). A stored record that is
        // not what its ID says is answered as an ID collision instead.
        if (collisions.length === 0) {
          const excluded = lib.excludedFromBom(part.revision.parsed, markOf);
          if ((part.revision.parsed.excluded_from_bom === true) !== excluded) {
            fail(
              excluded
                ? `the revision of ${part.ipn} must be Excluded from BOM: a symbol it uses says (in_bom no), or every footprint choice has exclude_from_bom`
                : `the revision of ${part.ipn} cannot be Excluded from BOM: no symbol it uses says (in_bom no), and not every footprint choice has exclude_from_bom`,
            );
          }
        }
        if (!stored) {
          part.revisionAfter = 1;
          part.detailsAfter = 1;
          newParts.push(part);
          continue;
        }
        if (stored.getString("ipn") !== part.ipn) {
          fail(`part ${part.id} has IPN ${stored.getString("ipn")}, not ${part.ipn}`);
        }
        const currentDetails = byId("catalog_part_details", stored.getString("current_details"));
        if (!currentRevision || !currentDetails) {
          fail(`part ${part.ipn} has no current revision or details`);
        }
        const location = `rows[IPN=${part.ipn}]`;
        const row = part.parsedSource;
        const revisionNumber = currentRevision.getInt("number");
        const detailsNumber = currentDetails.getInt("number");
        part.revisionAfter = revisionNumber;
        part.detailsAfter = detailsNumber;
        const sameDetails = currentDetails.getString("snapshot_sha256") === part.details.sha;
        const sameRevision = currentRevision.getString("snapshot_sha256") === part.revision.sha;
        const detailsSavedBy = savedBy(currentDetails);
        const revisionSavedBy = savedBy(currentRevision);

        if (sameDetails && sameRevision) {
          unchangedParts++;
          // The row the part was last saved from: the source of whichever of
          // its current details and revision was saved later.
          const newer = detailsSavedBy.seq >= revisionSavedBy.seq ? currentDetails : currentRevision;
          let previousSource = null;
          try {
            previousSource = JSON.parse(newer.getString("source"));
          } catch (_) {
            previousSource = null;
          }
          if (!previousSource || previousSource.source !== lib.CSV_CATALOG_SOURCE || typeof previousSource.raw_record !== "string") {
            serverProblems.push(
              problem(
                "part-last-saved-without-csv-source",
                part.ipn,
                row.path || "",
                location,
                `${part.ipn} was last saved by an edit, so there is no CSV row to compare the row with; the row is kept here and nothing is saved`,
                JSON.stringify({
                  current_revision_number: revisionNumber,
                  current_details_number: detailsNumber,
                  path: row.path,
                  row_number: row.row_number,
                  raw_record: row.raw_record,
                  substitutes_csv_rows: substitutesCsvRows(row),
                }),
              ),
            );
          } else if (
            previousSource.raw_record !== row.raw_record ||
            JSON.stringify(substitutesCsvRows(previousSource)) !== JSON.stringify(substitutesCsvRows(row))
          ) {
            // The part's row or one of its substitutes.csv rows changed while
            // neither its details nor its CAD did, so only supplier data
            // changed (such as an LCSC code or a SupplierPN).
            serverProblems.push(
              problem(
                "csv-row-changed-without-part-details-change",
                part.ipn,
                row.path || "",
                location,
                `the CSV rows of ${part.ipn} changed but neither its part details nor its symbols, footprints and 3D models did (for example only its supplier data); the new rows are kept here and nothing is saved`,
                // Only the rows identify it, so a later file version with
                // the same rows adds nothing.
                JSON.stringify({
                  current_revision_number: revisionNumber,
                  current_details_number: detailsNumber,
                  path: row.path,
                  row_number: row.row_number,
                  previous_raw_record: previousSource.raw_record,
                  raw_record: row.raw_record,
                  previous_substitutes_csv_rows: substitutesCsvRows(previousSource),
                  substitutes_csv_rows: substitutesCsvRows(row),
                }),
              ),
            );
          }
          continue;
        }

        if (!sameDetails) {
          if (detailsSavedBy.kind === lib.CHANGE_KINDS.csvImport) {
            part.newDetails = { number: detailsNumber + 1, predecessor: currentDetails.id };
            part.detailsAfter = detailsNumber + 1;
          } else {
            serverProblems.push(
              problem(
                "csv-details-differ-from-edited-details",
                part.ipn,
                row.path || "",
                location,
                `the part details in the row of ${part.ipn} differ from its current details, which were saved by an edit, not a CSV import; the row is kept here and the details are left as they are`,
                JSON.stringify({
                  current_details_number: detailsNumber,
                  current_details_change_kind: detailsSavedBy.kind,
                  path: row.path,
                  row_number: row.row_number,
                  raw_record: row.raw_record,
                  substitutes_csv_rows: substitutesCsvRows(row),
                  details: part.details.snapshot,
                }),
              ),
            );
          }
        }
        // The part's CAD content as this row would save it: CAD item
        // revisions this request adds are not stored yet, so their content
        // comes from what the request makes current.
        const storedContent = lib.storedCadContent(txApp);
        const archivedMatch = sameRevision
          ? 0
          : lib.archivedRevisionWithContent(
              txApp,
              part.id,
              lib.partRevisionContent(part.revision.parsed, (target) => {
                const after = currentAfter[target.cadItem || target.cad_item];
                return after && after.number === target.number ? after.content : storedContent(target);
              }),
            );
        if (archivedMatch > 0) {
          serverProblems.push(
            problem(
              "csv-cad-matches-archived-revision",
              part.ipn,
              row.path || "",
              location,
              `the symbols, footprints or 3D models that the row of ${part.ipn} uses are those of its archived revision ${archivedMatch}; an import never brings an archived revision back, so the row is kept here and the current revision ${revisionNumber} is left as it is`,
              JSON.stringify({
                current_revision_number: revisionNumber,
                archived_revision_number: archivedMatch,
                path: row.path,
                row_number: row.row_number,
                raw_record: row.raw_record,
                substitutes_csv_rows: substitutesCsvRows(row),
                revision: part.revision.snapshot,
              }),
            ),
          );
        } else if (!sameRevision) {
          if (revisionSavedBy.kind === lib.CHANGE_KINDS.csvImport) {
            // After the newest revision, which may be archived.
            const newest = lib.newestRevision(txApp, part.id);
            part.newRevision = { number: newest.getInt("number") + 1, predecessor: newest.id };
            part.revisionAfter = newest.getInt("number") + 1;
          } else {
            serverProblems.push(
              problem(
                "csv-cad-differs-from-edited-revision",
                part.ipn,
                row.path || "",
                location,
                `the symbols, footprints or 3D models that the row of ${part.ipn} uses differ from its current revision ${revisionNumber}, which was saved by an edit, not a CSV import; the row is kept here and the current revision is left as it is`,
                JSON.stringify({
                  current_revision_number: revisionNumber,
                  current_revision_change_kind: revisionSavedBy.kind,
                  path: row.path,
                  row_number: row.row_number,
                  raw_record: row.raw_record,
                  substitutes_csv_rows: substitutesCsvRows(row),
                  revision: part.revision.snapshot,
                }),
              ),
            );
          }
        }
        if (part.newDetails || part.newRevision) {
          part.stored = stored;
          updatedParts.push(part);
        }
      }

      // A part an earlier import run of this CSV file made, but no row makes
      // now, keeps its revisions; an import problem says it is gone. Every
      // run finds it again (the problem's detail does not name the run), so
      // the latest run always lists it.
      const removedParts = [];
      if (previousRun) {
        const earlier = []
          .concat(Array.isArray(previousRun.parts) ? previousRun.parts.map((part) => part && part.ipn) : [])
          .concat(Array.isArray(previousRun.removed_parts) ? previousRun.removed_parts : []);
        for (const ipn of earlier) {
          if (typeof ipn !== "string" || seenIpns[ipn] || removedParts.indexOf(ipn) >= 0) {
            continue;
          }
          removedParts.push(ipn);
          const stored = byId("catalog_parts", lib.deriveId("part", ipn));
          serverProblems.push(
            problem(
              "part-not-in-csv-file",
              ipn,
              catalogCsv.path,
              `rows[IPN=${ipn}]`,
              `${ipn} was a row of ${catalogCsv.path} at an earlier import, but no row of it makes that part now; the part and its revisions are kept as they are`,
              JSON.stringify({
                path: catalogCsv.path,
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
        cadItems: cadItemRevisions.length - newCadItems.length,
        cadItemRevisions: cadItemRevisions.length - newCadItemRevisions.length,
        parts: unchangedParts,
        problems: Object.keys(seenProblems).length - newProblems.length,
      };
      const withNewRevision = updatedParts.filter((part) => part.newRevision).length;
      const withNewDetails = updatedParts.filter((part) => part.newDetails).length;
      const created = {
        blobs: newBlobs.length,
        cadItems: newCadItems.length,
        cadItemRevisions: newCadItemRevisions.length,
        parts: newParts.length,
        partRevisions: newParts.length + withNewRevision,
        updatedParts: withNewRevision,
        partDetails: newParts.length + withNewDetails,
        updatedDetails: withNewDetails,
        problems: newProblems.length,
      };
      // New blobs and CAD items only ever arrive with a new CAD item revision.
      const revisionCount = created.partRevisions + created.partDetails + created.cadItemRevisions;

      // Whether the previous import run of this CSV file read exactly these
      // files. If not, this run is recorded even when it adds nothing else,
      // so the latest run always holds what the CSV says now.
      const fileHashes = JSON.stringify(
        sources
          .map((source) => [source.path, source.byte_sha256])
          .sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)),
      );
      const sameFilesAsPreviousRun = previousEntry !== null && previousEntry.getString("file_hashes") === fileHashes;

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

        // A Change only when there is a revision or saved details to put in
        // it.
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
        audit.set("allowed_uncommitted_changes", allowUncommittedChanges);
        audit.set("file_hashes", fileHashes);
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
            parts: parts.map((part) => ({ ipn: part.ipn, revision: part.revisionAfter, details: part.detailsAfter })),
            // Parts earlier runs made that no row makes now.
            removed_parts: removedParts,
            // Problems are shared by content, so a report may count only the
            // ones this run found.
            problems: Object.keys(seenProblems).sort(),
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
          made.set("snapshot", part.revision.snapshot);
          made.set("snapshot_sha256", part.revision.sha);
          made.set("source", part.source);
          made.set("cad_item_revisions", part.bound);
          txApp.save(made);
          return revisionId;
        };
        const savePartDetails = (part, number, predecessor) => {
          const detailsId = lib.deriveId("part-details", part.id + "\n" + number);
          const made = record("catalog_part_details", detailsId);
          made.set("part", part.id);
          made.set("number", number);
          made.set("predecessor", predecessor);
          made.set("change", changeId);
          made.set("snapshot", part.details.snapshot);
          made.set("snapshot_sha256", part.details.sha);
          made.set("source", part.source);
          txApp.save(made);
          return detailsId;
        };
        for (const part of newParts) {
          const made = record("catalog_parts", part.id);
          made.set("ipn", part.ipn);
          made.set("ipn_prefix", part.ipn.substring(0, 3));
          made.set("variant", part.ipn.substring(9));
          made.set("current_revision_number", 1);
          made.set("current_details_number", 1);
          made.set("first_change", changeId);
          txApp.save(made);
          made.set("current_details", savePartDetails(part, 1, ""));
          made.set("current_revision", savePartRevision(part, 1, ""));
          txApp.save(made);
        }
        for (const part of updatedParts) {
          if (part.newDetails) {
            part.stored.set("current_details", savePartDetails(part, part.newDetails.number, part.newDetails.predecessor));
            part.stored.set("current_details_number", part.newDetails.number);
          }
          if (part.newRevision) {
            part.stored.set("current_revision", savePartRevision(part, part.newRevision.number, part.newRevision.predecessor));
            part.stored.set("current_revision_number", part.newRevision.number);
          }
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
  $apis.bodyLimit(IMPORT_BODY_LIMIT),
  $apis.requireSuperuserAuth(),
);

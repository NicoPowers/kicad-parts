/// <reference path="../pb_data/types.d.ts" />

// POST /api/catalog/legacy-imports: publishes one Legacy Catalog import as
// one Change Set. Superusers only. The request is built by
// crates/catalog (LegacyImport + pocketbase::ImportCommand):
//
//   { correlationId, reason, legacyGrouping, sourceCommit,
//     sources: [{ role, path, byteSha256, size, headerRaw }],
//     blobs: [{ sha256, size, base64 }],
//     assetRevisions: [{ id, assetId, kind, key, revisionSha256,
//                        declaration, provenance }],
//     parts: [{ id, ipn, snapshot, snapshotSha256, provenance }],
//     diagnostics: [{ id, code, subject, path, location, message, detail }] }
//
// The server trusts none of the derived values: it re-hashes every blob,
// declaration and snapshot, re-derives every record ID, and checks that each
// binding names an Asset Revision that exists or arrives in this request.
//
// Then, in one transaction, it compares the request with what is stored:
// - blobs, assets, Asset Revisions and diagnostics that already exist are
//   reused (they are content-addressed);
// - a new IPN becomes a Managed Part with Part Revision 1;
// - an IPN whose head has the same snapshot hash is unchanged; if its source
//   row's bytes changed anyway (for example an Operational Fact such as a
//   price), an Import Diagnostic keeps the new row;
// - an IPN whose snapshot changed gets a successor revision if its head was
//   itself published by a legacy import, and otherwise an Import Diagnostic
//   instead, so a re-import never overwrites an edit made in the Catalog.
//
// Answers:
//   200 { outcome: "applied", changeSetId, seq, created, unchanged }
//   422 { outcome: "noop", unchanged }    everything is already stored; no
//                                          Change Set is created
//   409 { outcome: "correlation_reused" } the ID was used for another request
//   400 bad or inconsistent input, 401/403 not a superuser.
// Applied and no-op answers are stored under the correlation ID and replayed
// byte for byte (with X-Collab-Replayed: true) without writing anything.
routerAdd(
  "POST",
  "/api/catalog/legacy-imports",
  (e) => {
    const lib = require(`${__hooks}/catalog_lib.js`);
    const body = e.requestInfo().body || {};
    const actor = "_superusers:" + e.auth.id;

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
    const legacyGrouping = text(body, "legacyGrouping", 100, true);
    const sourceCommit = text(body, "sourceCommit", 100, false);
    const DOCUMENT = 1000000;

    // ---- Validate the request on its own -------------------------------
    const sources = list("sources").map((source) => ({
      role: text(source, "role", 50, true),
      path: text(source, "path", 500, true),
      byte_sha256: isSha(source.byteSha256) ? source.byteSha256 : fail("source byteSha256 is invalid"),
      size: Number.isInteger(source.size) && source.size >= 0 ? source.size : fail("source size is invalid"),
      header_raw: source.headerRaw === null || source.headerRaw === undefined ? null : text(source, "headerRaw", DOCUMENT, false),
    }));
    if (sources.length === 0) {
      fail("sources must not be empty");
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
      return { id: lib.deriveId("asset-blob", blob.sha256), sha256: blob.sha256, bytes: bytes };
    });
    const requestBlobs = {};
    for (const blob of blobs) {
      if (requestBlobs[blob.sha256]) {
        fail(`blob ${blob.sha256} appears twice`);
      }
      requestBlobs[blob.sha256] = blob;
    }

    const KINDS = ["symbol", "footprint", "model-3d"];
    const assetRevisions = list("assetRevisions").map((revision) => {
      const kind = text(revision, "kind", 20, true);
      if (KINDS.indexOf(kind) < 0) {
        fail(`asset kind ${kind} is unknown`);
      }
      const key = text(revision, "key", 500, true);
      const assetId = lib.deriveId("catalog-asset", kind + "\n" + key);
      if (revision.assetId !== assetId) {
        fail(`asset ID for ${kind} ${key} must be ${assetId}`);
      }
      const declaration = text(revision, "declaration", DOCUMENT, true);
      const revisionSha256 = $security.sha256(declaration);
      if (revision.revisionSha256 !== revisionSha256) {
        fail(`revisionSha256 of ${kind} ${key} does not match its declaration`);
      }
      const id = lib.deriveId("asset-revision", assetId + "\n" + revisionSha256);
      if (revision.id !== id) {
        fail(`asset revision ID for ${kind} ${key} must be ${id}`);
      }
      const parsed = parse(declaration, "declaration");
      if (parsed.kind !== kind || parsed.key !== key || !isSha(parsed.blob_sha256)) {
        fail(`declaration of ${kind} ${key} disagrees with its revision`);
      }
      const license = parsed.license && typeof parsed.license.status === "string" ? parsed.license.status : "";
      if (license.length === 0) {
        fail(`declaration of ${kind} ${key} has no license declaration`);
      }
      return {
        id: id,
        assetId: assetId,
        kind: kind,
        key: key,
        revisionSha256: revisionSha256,
        declaration: declaration,
        parsed: parsed,
        license: license,
        provenance: text(revision, "provenance", DOCUMENT, true),
      };
    });
    const requestRevisions = {};
    for (const revision of assetRevisions) {
      if (requestRevisions[revision.id]) {
        fail(`asset revision ${revision.id} appears twice`);
      }
      requestRevisions[revision.id] = revision;
    }

    const parts = list("parts").map((part) => {
      const ipn = text(part, "ipn", 13, true);
      if (!/^[A-Z]{3}-[0-9]{4}-[0-9A-Z]{4}$/.test(ipn)) {
        fail(`${ipn} is not an IPN`);
      }
      const id = lib.deriveId("managed-part", ipn);
      if (part.id !== id) {
        fail(`Managed Part ID for ${ipn} must be ${id}`);
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
      const provenance = text(part, "provenance", DOCUMENT, true);
      const bound = [];
      for (const binding of parsed.asset_bindings || []) {
        if (binding.target && binding.target.kind === "asset-revision") {
          bound.push(lib.deriveId("asset-revision", binding.target.asset + "\n" + binding.target.asset_revision_sha256));
        }
      }
      return {
        id: id,
        ipn: ipn,
        snapshot: snapshot,
        snapshotSha256: snapshotSha256,
        provenance: provenance,
        parsedProvenance: parse(provenance, "provenance"),
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

    const diagnostic = (code, subject, path, location, message, detail) => ({
      id: lib.deriveId("import-diagnostic", code + "\n" + subject + "\n" + location + "\n" + $security.sha256(detail)),
      code: code,
      subject: subject,
      path: path,
      location: location,
      message: message,
      detail: detail,
    });
    const diagnostics = list("diagnostics").map((item) => {
      const built = diagnostic(
        text(item, "code", 100, true),
        text(item, "subject", 500, true),
        text(item, "path", 500, true),
        text(item, "location", 2000, true),
        text(item, "message", 10000, true),
        text(item, "detail", DOCUMENT, true),
      );
      if (item.id !== built.id) {
        fail(`diagnostic ID for ${built.code} at ${built.location} must be ${built.id}`);
      }
      return built;
    });

    // The request's identity, built from validated values in a fixed order
    // (the parsed body is a Go map, whose key order varies).
    const requestHash = $security.sha256(
      JSON.stringify([
        reason,
        legacyGrouping,
        sourceCommit,
        sources.map((source) => [source.role, source.path, source.byte_sha256, source.size, source.header_raw]),
        blobs.map((blob) => blob.sha256),
        assetRevisions.map((revision) => [revision.id, revision.provenance]),
        parts.map((part) => [part.id, part.snapshotSha256, part.provenance]),
        diagnostics.map((item) => [item.id, item.path, item.message]),
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

      // ---- Phase 1: decide what is new, writing nothing ----------------
      const newBlobs = blobs.filter((blob) => !byId("catalog_asset_blobs", blob.id));
      const blobExists = (sha) => !!requestBlobs[sha] || !!byId("catalog_asset_blobs", lib.deriveId("asset-blob", sha));
      const revisionExists = (id) => !!requestRevisions[id] || !!byId("catalog_asset_revisions", id);

      const newAssets = [];
      const newAssetRevisions = [];
      const pendingNumbers = {};
      const seenAssets = {};
      for (const revision of assetRevisions) {
        if (!blobExists(revision.parsed.blob_sha256)) {
          fail(`asset revision ${revision.id} names blob ${revision.parsed.blob_sha256}, which is neither stored nor sent`);
        }
        for (const model of revision.parsed.model_references || []) {
          if (model.target && model.target.kind === "asset-revision") {
            const target = lib.deriveId("asset-revision", model.target.asset + "\n" + model.target.asset_revision_sha256);
            if (!revisionExists(target)) {
              fail(`${revision.key} binds 3D model revision ${target}, which is neither stored nor sent`);
            }
          }
        }
        if (!seenAssets[revision.assetId] && !byId("catalog_assets", revision.assetId)) {
          newAssets.push(revision);
        }
        seenAssets[revision.assetId] = true;
        if (byId("catalog_asset_revisions", revision.id)) {
          continue;
        }
        let latest = pendingNumbers[revision.assetId];
        if (!latest) {
          const stored = first("catalog_asset_revisions", "asset = {:asset}", { asset: revision.assetId }, "-number");
          latest = stored ? { number: stored.getInt("number"), id: stored.id } : { number: 0, id: "" };
        }
        revision.number = latest.number + 1;
        revision.predecessor = latest.id;
        pendingNumbers[revision.assetId] = { number: revision.number, id: revision.id };
        newAssetRevisions.push(revision);
      }

      const newParts = [];
      const successors = [];
      const serverDiagnostics = [];
      let unchangedParts = 0;
      for (const part of parts) {
        for (const bound of part.bound) {
          if (!revisionExists(bound)) {
            fail(`${part.ipn} binds Asset Revision ${bound}, which is neither stored nor sent`);
          }
        }
        const stored = byId("catalog_managed_parts", part.id);
        if (!stored) {
          newParts.push(part);
          continue;
        }
        if (stored.getString("ipn") !== part.ipn) {
          fail(`Managed Part ${part.id} has IPN ${stored.getString("ipn")}, not ${part.ipn}`);
        }
        const head = byId("catalog_part_revisions", stored.getString("head"));
        if (!head) {
          fail(`Managed Part ${part.ipn} has no head revision`);
        }
        const location = `rows[IPN=${part.ipn}]`;
        if (head.getString("snapshot_sha256") === part.snapshotSha256) {
          unchangedParts++;
          const previous = JSON.parse(head.getString("provenance"));
          const current = part.parsedProvenance;
          if (previous.raw_record !== current.raw_record) {
            serverDiagnostics.push(
              diagnostic(
                "legacy-row-changed-without-semantic-change",
                part.ipn,
                current.path || "",
                location,
                `the source row of ${part.ipn} changed but its Part Revision snapshot did not (for example an Operational Fact); the new row is kept here and no revision is created`,
                // Only the rows identify it, so a later file version with
                // the same row adds nothing.
                JSON.stringify({
                  head_number: head.getInt("number"),
                  path: current.path,
                  row_number: current.row_number,
                  previous_raw_record: previous.raw_record,
                  raw_record: current.raw_record,
                }),
              ),
            );
          }
          continue;
        }
        const headChangeSet = byId("catalog_change_sets", head.getString("change_set"));
        if (headChangeSet && headChangeSet.getString("kind") === "legacy-import") {
          part.number = head.getInt("number") + 1;
          part.predecessor = head.id;
          part.stored = stored;
          successors.push(part);
        } else {
          serverDiagnostics.push(
            diagnostic(
              "legacy-row-conflicts-with-head",
              part.ipn,
              part.parsedProvenance.path || "",
              location,
              `the source row of ${part.ipn} changed, but its head revision ${head.getInt("number")} was not published by a legacy import; the row is kept here and the head is left as it is`,
              JSON.stringify({
                head_number: head.getInt("number"),
                head_change_set_kind: headChangeSet ? headChangeSet.getString("kind") : "",
                path: part.parsedProvenance.path,
                row_number: part.parsedProvenance.row_number,
                raw_record: part.parsedProvenance.raw_record,
                snapshot: part.snapshot,
              }),
            ),
          );
        }
      }

      const newDiagnostics = [];
      const seenDiagnostics = {};
      for (const item of diagnostics.concat(serverDiagnostics)) {
        if (!seenDiagnostics[item.id] && !byId("catalog_import_diagnostics", item.id)) {
          newDiagnostics.push(item);
        }
        seenDiagnostics[item.id] = true;
      }

      const unchanged = {
        blobs: blobs.length - newBlobs.length,
        assets: Object.keys(seenAssets).length - newAssets.length,
        assetRevisions: assetRevisions.length - newAssetRevisions.length,
        managedParts: unchangedParts,
        diagnostics: Object.keys(seenDiagnostics).length - newDiagnostics.length,
      };
      let status;
      let result;
      const nothingNew =
        newBlobs.length + newAssets.length + newAssetRevisions.length + newParts.length + successors.length + newDiagnostics.length === 0;
      if (nothingNew) {
        status = 422;
        result = { outcome: "noop", correlationId: correlationId, unchanged: unchanged };
      } else {
        // ---- Phase 2: publish one Change Set ---------------------------
        const created = {
          blobs: newBlobs.length,
          assets: newAssets.length,
          assetRevisions: newAssetRevisions.length,
          managedParts: newParts.length,
          partRevisions: newParts.length + successors.length,
          successorRevisions: successors.length,
          diagnostics: newDiagnostics.length,
        };
        const last = first("catalog_change_sets", "seq > 0", {}, "-seq");
        const seq = last ? last.getInt("seq") + 1 : 1;
        const changeSetId = lib.deriveId("change-set", correlationId);
        const collection = (name) => txApp.findCollectionByNameOrId(name);
        const record = (name, id) => {
          const made = new Record(collection(name));
          made.set("id", id);
          return made;
        };

        const changeSet = record("catalog_change_sets", changeSetId);
        changeSet.set("seq", seq);
        changeSet.set("kind", "legacy-import");
        changeSet.set("reason", reason);
        changeSet.set("correlation_id", correlationId);
        changeSet.set("actor", actor);
        changeSet.set("source_commit", sourceCommit);
        changeSet.set("sources", JSON.stringify({ legacy_grouping: legacyGrouping, files: sources }));
        changeSet.set("summary", JSON.stringify(created));
        txApp.save(changeSet);

        for (const blob of newBlobs) {
          const made = record("catalog_asset_blobs", blob.id);
          made.set("sha256", blob.sha256);
          made.set("size", blob.bytes.length);
          made.set("file", $filesystem.fileFromBytes(blob.bytes, blob.sha256 + ".bin"));
          made.set("first_change_set", changeSetId);
          txApp.save(made);
        }
        for (const asset of newAssets) {
          const made = record("catalog_assets", asset.assetId);
          made.set("kind", asset.kind);
          made.set("key", asset.key);
          made.set("first_change_set", changeSetId);
          txApp.save(made);
        }
        for (const revision of newAssetRevisions) {
          const made = record("catalog_asset_revisions", revision.id);
          made.set("asset", revision.assetId);
          made.set("number", revision.number);
          made.set("predecessor", revision.predecessor);
          made.set("change_set", changeSetId);
          made.set("blob", lib.deriveId("asset-blob", revision.parsed.blob_sha256));
          made.set("revision_sha256", revision.revisionSha256);
          made.set("declaration", revision.declaration);
          made.set("license_status", revision.license);
          made.set("provenance", revision.provenance);
          txApp.save(made);
        }
        const saveRevision = (part, number, predecessor) => {
          const revisionId = lib.deriveId("part-revision", part.id + "\n" + number);
          const made = record("catalog_part_revisions", revisionId);
          made.set("managed_part", part.id);
          made.set("number", number);
          made.set("predecessor", predecessor);
          made.set("change_set", changeSetId);
          made.set("snapshot", part.snapshot);
          made.set("snapshot_sha256", part.snapshotSha256);
          made.set("provenance", part.provenance);
          made.set("asset_revisions", part.bound);
          txApp.save(made);
          return revisionId;
        };
        for (const part of newParts) {
          const made = record("catalog_managed_parts", part.id);
          made.set("ipn", part.ipn);
          made.set("ipn_family", part.ipn.substring(0, 3));
          made.set("ipn_base", part.ipn.substring(0, 8));
          made.set("variation", part.ipn.substring(9));
          made.set("head_number", 1);
          made.set("first_change_set", changeSetId);
          txApp.save(made);
          made.set("head", saveRevision(part, 1, ""));
          txApp.save(made);
        }
        for (const part of successors) {
          const head = saveRevision(part, part.number, part.predecessor);
          part.stored.set("head", head);
          part.stored.set("head_number", part.number);
          txApp.save(part.stored);
        }
        for (const item of newDiagnostics) {
          const made = record("catalog_import_diagnostics", item.id);
          made.set("code", item.code);
          made.set("subject", item.subject);
          made.set("path", item.path);
          made.set("location", item.location);
          made.set("message", item.message);
          made.set("detail", item.detail);
          made.set("change_set", changeSetId);
          txApp.save(made);
        }
        const audit = record("catalog_audit_events", lib.deriveId("audit-event", changeSetId));
        audit.set("action", "catalog.legacy-import");
        audit.set("actor", actor);
        audit.set("change_set", changeSetId);
        audit.set("reason", reason);
        audit.set("correlation_id", correlationId);
        txApp.save(audit);

        status = 200;
        result = {
          outcome: "applied",
          correlationId: correlationId,
          changeSetId: changeSetId,
          seq: seq,
          created: created,
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

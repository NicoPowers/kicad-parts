/// <reference path="../pb_data/types.d.ts" />

// POST /api/collab/commands: the only way a user writes a collab_records row.
//
// Request body:
//   { "correlationId": "...", "recordKey": "...", "expectedRevision": 0,
//     "title": "...", "content": "..." }
//
// expectedRevision is the revision the caller last saw; 0 means "the record
// must not exist yet". The command runs in one transaction and answers:
//   200 { outcome: "applied", ..., revision, seq }   the change was written
//   422 { outcome: "noop", ..., revision }           rejected: nothing would
//                                                     change
//   409 { outcome: "conflict", ..., currentRevision } expectedRevision is stale
//   409 { outcome: "correlation_reused", ... }        the ID was used for a
//                                                     different command
//   400 bad input, 401 not signed in as a user.
// Applied and no-op outcomes are stored under the correlation ID. Repeating
// the same command with the same ID returns the stored status and body byte
// for byte (with an X-Collab-Replayed: true header) and writes nothing.
// Conflicts are not stored: a retry is evaluated again against the current
// revision, which keeps answering 409 until the caller reloads.
routerAdd(
  "POST",
  "/api/collab/commands",
  (e) => {
    const body = e.requestInfo().body || {};
    const actor = e.auth;

    const text = (name, max, required) => {
      const value = body[name];
      if (value === undefined || value === null) {
        if (required) {
          throw new BadRequestError(`${name} is required`);
        }
        return "";
      }
      if (typeof value !== "string") {
        throw new BadRequestError(`${name} must be a string`);
      }
      if (value.length > max) {
        throw new BadRequestError(`${name} is longer than ${max} characters`);
      }
      if (required && value.length === 0) {
        throw new BadRequestError(`${name} is required`);
      }
      return value;
    };

    const correlationId = text("correlationId", 100, true);
    if (!/^[A-Za-z0-9_.:-]+$/.test(correlationId)) {
      throw new BadRequestError("correlationId may only contain letters, digits and _ . : -");
    }
    const recordKey = text("recordKey", 200, true);
    const title = text("title", 500, false);
    const content = text("content", 20000, false);
    const expectedRevision = body.expectedRevision;
    if (
      typeof expectedRevision !== "number" ||
      !Number.isInteger(expectedRevision) ||
      expectedRevision < 0
    ) {
      throw new BadRequestError("expectedRevision must be a non-negative integer");
    }
    const requestHash = $security.sha256(
      JSON.stringify([recordKey, expectedRevision, title, content]),
    );

    let reply = null;
    $app.runInTransaction((txApp) => {
      const first = (collection, filter, params, sort) => {
        const found = txApp.findRecordsByFilter(collection, filter, sort || "", 1, 0, params);
        return found.length > 0 ? found[0] : null;
      };

      const prior = first("collab_commands", "correlation_id = {:id}", { id: correlationId });
      if (prior) {
        if (prior.getString("actor") !== actor.id || prior.getString("request_hash") !== requestHash) {
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
        reply = {
          status: prior.getInt("status"),
          body: JSON.parse(prior.getString("result")),
          replayed: true,
        };
        return;
      }

      let record = first("collab_records", "key = {:key}", { key: recordKey });
      const currentRevision = record ? record.getInt("revision") : 0;
      if (expectedRevision !== currentRevision) {
        reply = {
          status: 409,
          body: {
            outcome: "conflict",
            correlationId: correlationId,
            recordKey: recordKey,
            expectedRevision: expectedRevision,
            currentRevision: currentRevision,
            message: "the record changed since expectedRevision; reload and retry",
          },
        };
        return;
      }

      let status = 200;
      let result;
      if (record && record.getString("title") === title && record.getString("content") === content) {
        status = 422;
        result = {
          outcome: "noop",
          correlationId: correlationId,
          recordKey: recordKey,
          recordId: record.id,
          revision: currentRevision,
        };
      } else {
        if (!record) {
          record = new Record(txApp.findCollectionByNameOrId("collab_records"));
          record.set("key", recordKey);
        }
        const revision = currentRevision + 1;
        record.set("title", title);
        record.set("content", content);
        record.set("revision", revision);
        record.set("updated_by", actor.id);
        txApp.save(record);

        const last = first("collab_changes", "seq > 0", {}, "-seq");
        const seq = last ? last.getInt("seq") + 1 : 1;
        const change = new Record(txApp.findCollectionByNameOrId("collab_changes"));
        change.set("seq", seq);
        change.set("record", record.id);
        change.set("record_key", recordKey);
        change.set("revision", revision);
        change.set("correlation_id", correlationId);
        change.set("actor", actor.id);
        change.set("title", title);
        change.set("content", content);
        txApp.save(change);

        result = {
          outcome: "applied",
          correlationId: correlationId,
          recordKey: recordKey,
          recordId: record.id,
          revision: revision,
          seq: seq,
        };
      }

      const ledger = new Record(txApp.findCollectionByNameOrId("collab_commands"));
      ledger.set("correlation_id", correlationId);
      ledger.set("actor", actor.id);
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
  $apis.requireAuth("users"),
);

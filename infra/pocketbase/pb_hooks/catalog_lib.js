/// <reference path="../pb_data/types.d.ts" />

// Helpers for the Catalog routes, loaded with
// require(`${__hooks}/catalog_lib.js`) inside a handler. PocketBase does not
// load this file by itself (only *.pb.js files are hooks).

// The closed set of catalog_changes.kind values, shared with the
// migration (1791500000_catalog_init.js) and crates/catalog (ChangeKind);
// a test in crates/catalog keeps the three in step.
const CHANGE_KINDS = Object.freeze({
  csvImport: "csv-import",
  edit: "edit",
  restore: "restore",
});

// The `source` value of rows read from the CSV catalog.
const CSV_CATALOG_SOURCE = "csv-catalog";

// Schema tags shared with crates/catalog (PART_DETAILS_SCHEMA and
// SOURCE_SCHEMA); a test there keeps them in step.
const PART_DETAILS_SCHEMA = "kicad-parts/part-details/1";
const SOURCE_SCHEMA = "kicad-parts/source/1";

// Trims the characters Rust's str::trim removes (Unicode White_Space), so
// the server reads Symbol and Footprint references exactly as the CSV import
// (crates/catalog) and the desktop do. JavaScript's own trim() differs: it
// also removes U+FEFF (byte order mark) and keeps U+0085 (next line).
const SPACE = "[\\u0009-\\u000D\\u0020\\u0085\\u00A0\\u1680\\u2000-\\u200A\\u2028\\u2029\\u202F\\u205F\\u3000]";
const LEADING_SPACE = new RegExp("^" + SPACE + "+");
const TRAILING_SPACE = new RegExp(SPACE + "+$");

function trimSpace(text) {
  return text.replace(LEADING_SPACE, "").replace(TRAILING_SPACE, "");
}

// Words the desktop uses for authors who are not named team members. A
// member called one of these would look like someone else in the history.
const RESERVED_NAMES = ["an administrator", "someone"];

function isReservedName(name) {
  return RESERVED_NAMES.indexOf(trimSpace(String(name)).toLowerCase()) >= 0;
}

// ---- Collection guards ---------------------------------------------------
//
// The parts of a collection the Catalog depends on: its name and type, each
// field's ID, name and type, its rules, and its indexes. `source` is a
// Collection or the plain object of a collections import.
function collectionShape(source) {
  const plain = typeof source.marshalJSON === "function" ? JSON.parse(toString(source.marshalJSON())) : JSON.parse(JSON.stringify(source));
  const rule = (value) => (value === null || value === undefined ? null : String(value));
  return {
    id: plain.id || "",
    name: plain.name || "",
    type: plain.type || "",
    rules: {
      listRule: rule(plain.listRule),
      viewRule: rule(plain.viewRule),
      createRule: rule(plain.createRule),
      updateRule: rule(plain.updateRule),
      deleteRule: rule(plain.deleteRule),
    },
    fields: (plain.fields || []).map((field) => ({ id: field.id, name: field.name, type: field.type })),
    indexes: (plain.indexes || []).map((index) => String(index)),
  };
}

// The collections whose shape the Catalog depends on. For `users`, only
// the fields and rules the edit routes read are protected; the rest of the
// account settings stay editable in the dashboard.
function guardedCollection(name) {
  return name.indexOf("catalog_") === 0 || name === "users";
}

// Why changing collection `before` into `after` would lose or expose
// Catalog data, or "" if it would not. Allowed: adding fields and indexes,
// and making a rule stricter by setting it to superusers only (null).
// Refused: renaming the collection or changing its type; removing,
// renaming or retyping a field; dropping an index; any other rule change.
function collectionChangeProblem(before, after) {
  const old = collectionShape(before);
  const next = collectionShape(after);
  const users = old.name === "users";
  if (next.name !== old.name || next.type !== old.type) {
    return `${old.name} cannot be renamed or change type`;
  }
  const protectedFields = users ? ["email", "name", "can_edit_catalog"] : null;
  for (const field of old.fields) {
    if (protectedFields && protectedFields.indexOf(field.name) < 0) {
      continue;
    }
    const kept = next.fields.find((candidate) => candidate.id === field.id);
    if (!kept) {
      return `the field ${field.name} of ${old.name} cannot be removed`;
    }
    if (kept.name !== field.name || kept.type !== field.type) {
      return `the field ${field.name} of ${old.name} cannot be renamed or change type`;
    }
  }
  const rules = users ? ["createRule", "updateRule"] : Object.keys(old.rules);
  for (const name of rules) {
    if (next.rules[name] !== old.rules[name] && next.rules[name] !== null) {
      return `the ${name} of ${old.name} can only stay as it is or become superusers only`;
    }
  }
  if (!users) {
    for (const index of old.indexes) {
      if (next.indexes.indexOf(index) < 0) {
        return `an index of ${old.name} cannot be dropped: ${index}`;
      }
    }
  }
  return "";
}

// The Catalog's record ID rule, shared with crates/catalog/src/ids.rs:
// the first 15 hex digits of SHA-256("kicad-parts:<kind>:v1\n<key>").
function deriveId(kind, key) {
  return $security.sha256("kicad-parts:" + kind + ":v1\n" + key).substring(0, 15);
}

// $security.sha256 hashes text, which would mangle bytes that are not
// UTF-8, so blob bytes are hashed here.
const K = [
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

function sha256Bytes(bytes) {
  const length = bytes.length;
  const padded = new Uint8Array(((length + 9 + 63) >> 6) << 6);
  for (let i = 0; i < length; i++) {
    padded[i] = bytes[i];
  }
  padded[length] = 0x80;
  const bits = length * 8;
  const high = Math.floor(bits / 0x100000000);
  const end = padded.length;
  padded[end - 8] = (high >>> 24) & 0xff;
  padded[end - 7] = (high >>> 16) & 0xff;
  padded[end - 6] = (high >>> 8) & 0xff;
  padded[end - 5] = high & 0xff;
  padded[end - 4] = (bits >>> 24) & 0xff;
  padded[end - 3] = (bits >>> 16) & 0xff;
  padded[end - 2] = (bits >>> 8) & 0xff;
  padded[end - 1] = bits & 0xff;

  const h = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
  const w = new Array(64);
  for (let offset = 0; offset < end; offset += 64) {
    for (let t = 0; t < 16; t++) {
      const i = offset + t * 4;
      w[t] = ((padded[i] << 24) | (padded[i + 1] << 16) | (padded[i + 2] << 8) | padded[i + 3]) >>> 0;
    }
    for (let t = 16; t < 64; t++) {
      const x = w[t - 15];
      const y = w[t - 2];
      const s0 = ((x >>> 7) | (x << 25)) ^ ((x >>> 18) | (x << 14)) ^ (x >>> 3);
      const s1 = ((y >>> 17) | (y << 15)) ^ ((y >>> 19) | (y << 13)) ^ (y >>> 10);
      w[t] = (w[t - 16] + s0 + w[t - 7] + s1) >>> 0;
    }
    let a = h[0], b = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], k = h[7];
    for (let t = 0; t < 64; t++) {
      const S1 = ((e >>> 6) | (e << 26)) ^ ((e >>> 11) | (e << 21)) ^ ((e >>> 25) | (e << 7));
      const ch = (e & f) ^ (~e & g);
      const t1 = (k + S1 + ch + K[t] + w[t]) >>> 0;
      const S0 = ((a >>> 2) | (a << 30)) ^ ((a >>> 13) | (a << 19)) ^ ((a >>> 22) | (a << 10));
      const maj = (a & b) ^ (a & c) ^ (b & c);
      const t2 = (S0 + maj) >>> 0;
      k = g;
      g = f;
      f = e;
      e = (d + t1) >>> 0;
      d = c;
      c = b;
      b = a;
      a = (t1 + t2) >>> 0;
    }
    h[0] = (h[0] + a) >>> 0;
    h[1] = (h[1] + b) >>> 0;
    h[2] = (h[2] + c) >>> 0;
    h[3] = (h[3] + d) >>> 0;
    h[4] = (h[4] + e) >>> 0;
    h[5] = (h[5] + f) >>> 0;
    h[6] = (h[6] + g) >>> 0;
    h[7] = (h[7] + k) >>> 0;
  }
  let hex = "";
  for (let i = 0; i < 8; i++) {
    hex += ("00000000" + h[i].toString(16)).slice(-8);
  }
  return hex;
}

const BASE64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

// Strict standard base64 (with padding) to an array of byte values, or
// null if the text is not exactly that.
function base64Decode(text) {
  if (typeof text !== "string" || text.length % 4 !== 0) {
    return null;
  }
  const out = [];
  for (let i = 0; i < text.length; i += 4) {
    const chunk = [0, 0, 0, 0];
    let pad = 0;
    for (let j = 0; j < 4; j++) {
      const ch = text.charAt(i + j);
      if (ch === "=") {
        if (i + 4 !== text.length || j < 2) {
          return null;
        }
        pad++;
        continue;
      }
      if (pad > 0) {
        return null;
      }
      const value = BASE64.indexOf(ch);
      if (value < 0) {
        return null;
      }
      chunk[j] = value;
    }
    const triple = (chunk[0] << 18) | (chunk[1] << 12) | (chunk[2] << 6) | chunk[3];
    out.push((triple >> 16) & 0xff);
    if (pad < 2) {
      out.push((triple >> 8) & 0xff);
    }
    if (pad < 1) {
      out.push(triple & 0xff);
    }
  }
  return out;
}

// ---- Edits and restores --------------------------------------------------
//
// POST /api/catalog/edits and POST /api/catalog/restores (see
// catalog_edits.pb.js) publish one new revision of one part. Both run
// publishPartRevision in one transaction:
//
//   1. replay the stored answer if the correlation ID was used before for
//      the same request (409 correlation_reused for a different one);
//   2. answer 409 conflict, with the part's current revision, if the
//      caller expected another current revision;
//   3. build the new part details: for an edit, from the caller's fields
//      and manufacturer parts; for a restore, an exact copy of the older
//      revision's details, CAD item revisions included;
//   4. answer 422 noop if they equal the current revision's details;
//   5. otherwise save one Change, the new revision, the part's move to it,
//      and one audit entry.
//
// Applied and no-op answers are stored under the correlation ID; conflicts
// are not, so a retry is checked again against the current revision.

// Who may publish edits and restores: superusers, and team members whose
// account is marked can_edit_catalog. Returns the actor or throws.
function catalogEditor(e) {
  const auth = e.auth;
  const collection = auth.collection().name;
  if (collection === "users" && !auth.getBool("can_edit_catalog")) {
    throw new ForbiddenError("you can view the Catalog but not edit it; ask an admin for edit access");
  }
  // Only superusers can set a member's name (see 1791600000_catalog_edits.js),
  // so the name a Change shows is the one an administrator gave them.
  const given = auth.getString("name");
  const name =
    collection === "users" && given && !isReservedName(given) ? given : auth.getString("email");
  return { id: collection + ":" + auth.id, name: name };
}

// Reads and checks the request fields both routes share.
function commonRequest(body) {
  const fail = (message) => {
    throw new BadRequestError(message);
  };
  const text = (name, max) => {
    const value = body[name];
    if (typeof value !== "string" || value.length === 0) {
      fail(`${name} is required`);
    }
    if (value.length > max) {
      fail(`${name} is longer than ${max} characters`);
    }
    return value;
  };
  const correlationId = text("correlationId", 100);
  if (!/^[A-Za-z0-9_.:-]+$/.test(correlationId)) {
    fail("correlationId may only contain letters, digits and _ . : -");
  }
  const reason = text("reason", 2000);
  if (trimSpace(reason).length === 0) {
    fail("reason is required");
  }
  const ipn = text("ipn", 13);
  if (!/^[A-Z]{3}-[0-9]{4}-[0-9A-Z]{4}$/.test(ipn)) {
    fail(`${ipn} is not an IPN`);
  }
  const expectedRevision = body.expectedRevision;
  if (typeof expectedRevision !== "number" || !Number.isInteger(expectedRevision) || expectedRevision < 1) {
    fail("expectedRevision must be the number of the part's current revision");
  }
  return { correlationId: correlationId, reason: reason, ipn: ipn, expectedRevision: expectedRevision };
}

// The new fields and manufacturer parts of an edit, checked against the
// current revision: an edit may change field values, but never add, drop,
// rename or reorder fields, so supplier data (which is never a field)
// cannot enter the part details.
function editedDetails(body, currentDetails) {
  const fail = (message) => {
    throw new BadRequestError(message);
  };
  const fields = body.fields;
  if (!Array.isArray(fields)) {
    fail("fields must be a list");
  }
  const currentFields = currentDetails.fields || [];
  if (fields.length !== currentFields.length) {
    fail(`fields must list the same ${currentFields.length} fields as the current revision, in the same order`);
  }
  const checkedFields = fields.map((field, index) => {
    if (!field || field.name !== currentFields[index].name) {
      fail(`field ${index + 1} must be ${currentFields[index].name}; an edit cannot add, remove, rename or reorder fields`);
    }
    if (typeof field.value !== "string" || field.value.length > 10000) {
      fail(`the value of ${field.name} must be text of at most 10000 characters`);
    }
    return { name: field.name, value: field.value };
  });
  const parts = body.manufacturerParts;
  if (!Array.isArray(parts) || parts.length > 100) {
    fail("manufacturerParts must be a list of at most 100");
  }
  const seen = {};
  const checkedParts = parts.map((part) => {
    const manufacturer = part && typeof part.manufacturer === "string" ? part.manufacturer : "";
    const mpn = part && typeof part.mpn === "string" ? part.mpn : "";
    if (trimSpace(manufacturer).length === 0 || trimSpace(mpn).length === 0) {
      fail("every manufacturer part needs a manufacturer and an MPN");
    }
    if (manufacturer.length > 500 || mpn.length > 500) {
      fail("a manufacturer or MPN is longer than 500 characters");
    }
    const key = JSON.stringify([manufacturer, mpn]);
    if (seen[key]) {
      fail(`the manufacturer part ${manufacturer} ${mpn} is listed twice`);
    }
    seen[key] = true;
    return { manufacturer: manufacturer, mpn: mpn };
  });
  return { fields: checkedFields, manufacturerParts: checkedParts };
}

// The CAD references of new part details, read from their Symbol and
// Footprint fields the way the CSV import reads them (`;` separates
// footprint choices). Each reference to the CSV catalog's own libraries
// (`g-*`) is bound to one exact CAD item revision:
// - a reference the current revision already binds keeps its exact
//   revision, so editing another field never moves a CAD binding;
// - any other reference binds the current revision of the CAD item with
//   exactly that library reference;
// - a reference that the current revision kept as unresolved stays so;
// - anything else is refused: an edit can only use CAD items the Catalog
//   already has.
// Other libraries (such as KiCad's own) are external, as in the import.
function cadReferences(txApp, fields, currentDetails) {
  const value = (name) => {
    const field = fields.find((item) => item.name === name);
    return field ? field.value : null;
  };
  const latestRevision = (cadItemId) => {
    const found = txApp.findRecordsByFilter("catalog_cad_item_revisions", "cad_item = {:id}", "-number", 1, 0, {
      id: cadItemId,
    });
    return found.length > 0 ? found[0] : null;
  };
  const previous = currentDetails.cad_references || [];
  const references = [];
  for (const [role, column] of [["symbol", "Symbol"], ["footprint", "Footprint"]]) {
    const text = value(column);
    if (text === null) {
      continue;
    }
    const names = text
      .split(";")
      .map((name) => trimSpace(name))
      .filter((name) => name.length > 0);
    names.forEach((reference, ordinal) => {
      const library = reference.indexOf(":") >= 0 ? reference.substring(0, reference.indexOf(":")) : reference;
      let target;
      if (!library.startsWith("g-")) {
        target = { kind: "external" };
      } else {
        const cadItemId = deriveId("cad-item", role + "\n" + reference);
        let cadItem = null;
        try {
          cadItem = txApp.findRecordById("catalog_cad_items", cadItemId);
        } catch (_) {
          cadItem = null;
        }
        if (cadItem && (cadItem.getString("kind") !== role || cadItem.getString("key") !== reference)) {
          throw new BadRequestError(`the stored CAD item ${cadItemId} is not the ${role} ${reference}`);
        }
        const same = previous.find((item) => item.role === role && item.reference === reference);
        if (cadItem && same && same.target.kind === "cad-item-revision" && same.target.cad_item === cadItemId) {
          target = { kind: "cad-item-revision", cad_item: cadItemId, snapshot_sha256: same.target.snapshot_sha256 };
        } else if (cadItem) {
          const latest = latestRevision(cadItemId);
          if (!latest) {
            throw new BadRequestError(`the ${role} ${reference} has no revision`);
          }
          target = { kind: "cad-item-revision", cad_item: cadItemId, snapshot_sha256: latest.getString("snapshot_sha256") };
        } else if (same && same.target.kind === "unresolved") {
          target = { kind: "unresolved", status: same.target.status };
        } else {
          throw new BadRequestError(
            `the ${role} ${reference} is not in the Catalog; an edit can only use symbols and footprints the Catalog already has`,
          );
        }
      }
      references.push({ role: role, ordinal: ordinal, reference: reference, target: target });
    });
  }
  return references;
}

// The CAD item revision IDs a snapshot binds, for the relation field.
function boundCadItemRevisions(details) {
  const bound = [];
  for (const reference of details.cad_references || []) {
    if (reference.target && reference.target.kind === "cad-item-revision") {
      bound.push(deriveId("cad-item-revision", reference.target.cad_item + "\n" + reference.target.snapshot_sha256));
    }
  }
  return bound;
}

// What a client needs to show a revision someone else published.
function describeRevision(txApp, revision) {
  let change = null;
  try {
    change = txApp.findRecordById("catalog_changes", revision.getString("change"));
  } catch (_) {
    change = null;
  }
  return {
    id: revision.id,
    number: revision.getInt("number"),
    snapshot: revision.getString("snapshot"),
    snapshotSha256: revision.getString("snapshot_sha256"),
    changeKind: change ? change.getString("kind") : "",
    reason: change ? change.getString("reason") : "",
    actor: change ? change.getString("actor") : "",
    actorName: change ? change.getString("actor_name") : "",
    created: change ? change.getString("created") : "",
  };
}

// kind is CHANGE_KINDS.edit or CHANGE_KINDS.restore.
function publishPartRevision(e, kind) {
  const actor = catalogEditor(e);
  const body = e.requestInfo().body || {};
  const request = commonRequest(body);
  let restoreRevision = 0;
  if (kind === CHANGE_KINDS.restore) {
    restoreRevision = body.restoreRevision;
    if (typeof restoreRevision !== "number" || !Number.isInteger(restoreRevision) || restoreRevision < 1) {
      throw new BadRequestError("restoreRevision must be the number of an earlier revision");
    }
  } else if (!Array.isArray(body.fields) || !Array.isArray(body.manufacturerParts)) {
    throw new BadRequestError("an edit needs fields and manufacturerParts");
  }
  // The request's identity, from validated values in a fixed order (the
  // parsed body is a Go map, whose key order varies).
  const requestHash = $security.sha256(
    JSON.stringify([
      kind,
      request.ipn,
      request.expectedRevision,
      request.reason,
      kind === CHANGE_KINDS.restore
        ? restoreRevision
        : [
            body.fields.map((field) => [field && field.name, field && field.value]),
            body.manufacturerParts.map((part) => [part && part.manufacturer, part && part.mpn]),
          ],
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

    const prior = first("catalog_commands", "correlation_id = {:id}", { id: request.correlationId });
    if (prior) {
      if (prior.getString("actor") !== actor.id || prior.getString("request_hash") !== requestHash) {
        reply = {
          status: 409,
          body: {
            outcome: "correlation_reused",
            correlationId: request.correlationId,
            message: "this correlation ID was already used for a different command",
          },
        };
        return;
      }
      reply = { status: prior.getInt("status"), body: JSON.parse(prior.getString("result")), replayed: true };
      return;
    }

    const partId = deriveId("part", request.ipn);
    const part = byId("catalog_parts", partId);
    if (!part || part.getString("ipn") !== request.ipn) {
      throw new NotFoundError(`${request.ipn} is not a part in the Catalog`);
    }
    const current = byId("catalog_part_revisions", part.getString("current_revision"));
    if (!current) {
      throw new Error(`${request.ipn} has no current revision`);
    }
    if (request.expectedRevision !== current.getInt("number")) {
      reply = {
        status: 409,
        body: {
          outcome: "conflict",
          correlationId: request.correlationId,
          ipn: request.ipn,
          expectedRevision: request.expectedRevision,
          currentRevision: describeRevision(txApp, current),
          message: `someone else changed ${request.ipn} first: its current revision is ${current.getInt("number")}, not ${request.expectedRevision}`,
        },
      };
      return;
    }
    const currentDetails = JSON.parse(current.getString("snapshot"));
    if (currentDetails.schema !== PART_DETAILS_SCHEMA || currentDetails.ipn !== request.ipn) {
      throw new Error(`the current revision of ${request.ipn} is not ${PART_DETAILS_SCHEMA} part details`);
    }

    let snapshot;
    let source;
    let restored = null;
    if (kind === CHANGE_KINDS.restore) {
      restored = first("catalog_part_revisions", "part = {:part} && number = {:number}", {
        part: partId,
        number: restoreRevision,
      });
      if (!restored) {
        throw new NotFoundError(`${request.ipn} has no revision ${restoreRevision}`);
      }
      // The exact stored bytes, so the restored revision has the same
      // details and the same CAD item revisions.
      snapshot = restored.getString("snapshot");
      source = {
        schema: SOURCE_SCHEMA,
        source: CHANGE_KINDS.restore,
        restored_revision: { id: restored.id, number: restored.getInt("number") },
        previous_revision: { id: current.id, number: current.getInt("number") },
      };
    } else {
      const edited = editedDetails(body, currentDetails);
      // Serialized in the field order of crates/catalog PartDetails, so the
      // stored bytes are its canonical JSON.
      snapshot = JSON.stringify({
        schema: PART_DETAILS_SCHEMA,
        ipn: request.ipn,
        fields: edited.fields,
        manufacturer_parts: edited.manufacturerParts,
        cad_references: cadReferences(txApp, edited.fields, currentDetails),
      });
      source = {
        schema: SOURCE_SCHEMA,
        source: CHANGE_KINDS.edit,
        previous_revision: { id: current.id, number: current.getInt("number") },
      };
    }
    const snapshotSha256 = $security.sha256(snapshot);

    let status;
    let result;
    if (snapshotSha256 === current.getString("snapshot_sha256")) {
      status = 422;
      result = {
        outcome: "noop",
        correlationId: request.correlationId,
        ipn: request.ipn,
        revision: { id: current.id, number: current.getInt("number") },
        message:
          kind === CHANGE_KINDS.restore
            ? `revision ${restoreRevision} has the same details as the current revision`
            : "the edit changes nothing",
      };
    } else {
      const record = (name, id) => {
        const made = new Record(txApp.findCollectionByNameOrId(name));
        made.set("id", id);
        return made;
      };
      const last = first("catalog_changes", "seq > 0", {}, "-seq");
      const seq = last ? last.getInt("seq") + 1 : 1;
      const number = current.getInt("number") + 1;
      const changeId = deriveId("change", request.correlationId);
      const revisionId = deriveId("part-revision", partId + "\n" + number);
      const bound = boundCadItemRevisions(JSON.parse(snapshot));
      const summary = JSON.stringify({ ipn: request.ipn, partRevisions: 1, revision: number, restoredRevision: restoreRevision || null });

      const change = record("catalog_changes", changeId);
      change.set("seq", seq);
      change.set("kind", kind);
      change.set("reason", request.reason);
      change.set("correlation_id", request.correlationId);
      change.set("actor", actor.id);
      change.set("actor_name", actor.name);
      change.set("sources", JSON.stringify({ parts: [request.ipn] }));
      change.set("summary", summary);
      txApp.save(change);

      const revision = record("catalog_part_revisions", revisionId);
      revision.set("part", partId);
      revision.set("number", number);
      revision.set("predecessor", current.id);
      revision.set("change", changeId);
      revision.set("snapshot", snapshot);
      revision.set("snapshot_sha256", snapshotSha256);
      revision.set("source", JSON.stringify(source));
      revision.set("cad_item_revisions", bound);
      txApp.save(revision);

      part.set("current_revision", revisionId);
      part.set("current_revision_number", number);
      txApp.save(part);

      const auditEntryId = deriveId("audit-entry", request.correlationId);
      const audit = record("catalog_audit_entries", auditEntryId);
      audit.set("action", "catalog." + kind);
      audit.set("actor", actor.id);
      audit.set("actor_name", actor.name);
      audit.set("change", changeId);
      audit.set("reason", request.reason);
      audit.set("correlation_id", request.correlationId);
      audit.set("summary", summary);
      txApp.save(audit);

      status = 200;
      result = {
        outcome: "applied",
        correlationId: request.correlationId,
        ipn: request.ipn,
        changeId: changeId,
        seq: seq,
        auditEntryId: auditEntryId,
        revision: { id: revisionId, number: number, snapshotSha256: snapshotSha256 },
        cadItemRevisions: bound,
      };
    }

    const ledger = new Record(txApp.findCollectionByNameOrId("catalog_commands"));
    ledger.set("correlation_id", request.correlationId);
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
}

module.exports = {
  CHANGE_KINDS: CHANGE_KINDS,
  CSV_CATALOG_SOURCE: CSV_CATALOG_SOURCE,
  PART_DETAILS_SCHEMA: PART_DETAILS_SCHEMA,
  SOURCE_SCHEMA: SOURCE_SCHEMA,
  publishPartRevision: publishPartRevision,
  trimSpace: trimSpace,
  isReservedName: isReservedName,
  guardedCollection: guardedCollection,
  collectionChangeProblem: collectionChangeProblem,
  deriveId: deriveId,
  sha256Bytes: sha256Bytes,
  base64Decode: base64Decode,
};

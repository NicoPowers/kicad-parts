/// <reference path="../pb_data/types.d.ts" />

// Helpers for the Catalog routes, loaded with
// require(`${__hooks}/catalog_lib.js`) inside a handler. PocketBase does not
// load this file by itself (only *.pb.js files are hooks).

// The closed set of catalog_changes.kind values, shared with the
// migration (1791800000_shared_part_details.js) and crates/catalog
// (ChangeKind); a test in crates/catalog keeps the three in step.
const CHANGE_KINDS = Object.freeze({
  csvImport: "csv-import",
  edit: "edit",
  archive: "archive",
  unarchive: "unarchive",
});

// The `source` value of rows read from the CSV catalog.
const CSV_CATALOG_SOURCE = "csv-catalog";

// Schema tags shared with crates/catalog (PART_DETAILS_SCHEMA,
// PART_REVISION_SCHEMA and SOURCE_SCHEMA); a test there keeps them in step.
const PART_DETAILS_SCHEMA = "kicad-parts/part-details/2";
const PART_REVISION_SCHEMA = "kicad-parts/part-revision/1";
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

// Characters that take no visible space (zero-width spaces and joiners,
// direction marks, word joiners, variation selectors, filler letters, the
// byte order mark). In a name they would let "someone\u200B" pass as
// "someone", so names may not contain them.
const INVISIBLE = /[\u00AD\u034F\u061C\u115F\u1160\u17B4\u17B5\u180B-\u180F\u200B-\u200F\u202A-\u202E\u2060-\u206F\u3164\uFE00-\uFE0F\uFEFF\uFFA0]/g;

function hasInvisible(name) {
  return String(name).search(INVISIBLE) >= 0;
}

function isReservedName(name) {
  const visible = trimSpace(String(name).replace(INVISIBLE, "")).toLowerCase();
  return RESERVED_NAMES.indexOf(visible) >= 0;
}

// ---- Collection guards ---------------------------------------------------
//
// The parts of a collection the Catalog depends on: its name and type, the
// full definition of each field (every option, such as maxSelect, values,
// required and hidden, because PocketBase rewrites stored data in SQL when
// some of them change, without running any record hook), its rules, and
// its indexes. `source` is a Collection or the plain object of a
// collections import.
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
    fields: (plain.fields || []).map((field) => ({
      id: field.id,
      name: field.name,
      required: field.required === true,
      definition: canonicalJson(field),
    })),
    indexes: (plain.indexes || []).map((index) => String(index)),
  };
}

// JSON with object keys sorted at every level, so two definitions compare
// equal exactly when every option is equal.
function canonicalJson(value) {
  if (Array.isArray(value)) {
    return "[" + value.map(canonicalJson).join(",") + "]";
  }
  if (value !== null && typeof value === "object") {
    return (
      "{" +
      Object.keys(value)
        .sort()
        .map((key) => JSON.stringify(key) + ":" + canonicalJson(value[key]))
        .join(",") +
      "}"
    );
  }
  return JSON.stringify(value === undefined ? null : value);
}

// The collections whose shape the Catalog depends on. For `users`, only
// the fields and rules the edit routes read are protected; the rest of the
// account settings stay editable in the dashboard.
function guardedCollection(name) {
  return name.indexOf("catalog_") === 0 || name === "users";
}

// Why changing collection `before` into `after` would lose, expose or
// block Catalog data, or "" if it would not. Allowed: adding fields that
// are not required, adding indexes, and making a rule superusers only
// (null). Refused: renaming the collection or changing its type; removing
// a field or changing any option of it (name, type, maxSelect, select
// values, required, hidden, ...); adding a required field (every edit would
// then fail); dropping an index; any other rule change. For `users`, only
// email, name and can_edit_catalog and the create and update rules are
// checked.
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
    if (kept.definition !== field.definition) {
      return `the field ${field.name} of ${old.name} cannot be changed`;
    }
  }
  if (!users) {
    for (const field of next.fields) {
      const isNew = !old.fields.some((existing) => existing.id === field.id);
      if (isNew && field.required) {
        return `a required field (${field.name}) cannot be added to ${old.name}; existing routes would stop saving`;
      }
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

// ---- Edits ---------------------------------------------------------------
//
// POST /api/catalog/edits (see catalog_edits.pb.js) changes one part's
// details, its Symbol and Footprint, or both. It runs publishEdit in one
// transaction:
//
//   1. replay the stored answer if the correlation ID was used before for
//      the same request (409 correlation_reused for a different one);
//   2. answer 409 conflict if the part details or the revision a section
//      started from are no longer the current ones, giving the ones that
//      moved;
//   3. build the new part details from the details section, and the new
//      revision from the CAD section, binding exact CAD item revisions;
//   4. answer 422 noop if neither would change;
//   5. otherwise save one Change, the next part details and/or the next
//      revision, the part's move to them, and one audit entry.
//
// Applied and no-op answers are stored under the correlation ID; conflicts
// are not, so a retry is checked again against the current records.

// Who may publish edits: superusers, and team members whose account is
// marked can_edit_catalog. Returns the actor or throws.
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

function fail(message) {
  throw new BadRequestError(message);
}

// The correlation ID, reason and IPN every edit and archive request has,
// checked.
function commonRequest(body) {
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
  return { correlationId: correlationId, reason: reason, ipn: ipn };
}

// The checked request: correlation ID, reason, IPN, and the sections sent,
// each { expected, fields[, manufacturerParts] } or null.
function editRequest(body) {
  const { correlationId, reason, ipn } = commonRequest(body);
  const number = (section, name, what) => {
    const value = section[name];
    if (typeof value !== "number" || !Number.isInteger(value) || value < 1) {
      fail(`${name} must be the number of the part's current ${what}`);
    }
    return value;
  };
  const sent = (name) => {
    const section = body[name];
    if (section === undefined || section === null) {
      return null;
    }
    if (typeof section !== "object" || Array.isArray(section)) {
      fail(`${name} must be an object`);
    }
    return section;
  };
  let details = sent("details");
  let cad = sent("cad");
  if (!details && !cad) {
    fail("an edit needs details, cad or both");
  }
  if (details) {
    if (!Array.isArray(details.fields) || !Array.isArray(details.manufacturerParts)) {
      fail("details need fields and manufacturerParts");
    }
    details = {
      expected: number(details, "expectedDetails", "details"),
      fields: details.fields,
      manufacturerParts: details.manufacturerParts,
    };
  }
  if (cad) {
    if (!Array.isArray(cad.fields)) {
      fail("cad needs fields");
    }
    cad = { expected: number(cad, "expectedRevision", "revision"), fields: cad.fields };
  }
  return { correlationId: correlationId, reason: reason, ipn: ipn, details: details, cad: cad };
}

// New field values checked against the current ones: an edit may change
// values, but never add, drop, rename or reorder fields, so supplier data
// (never a field) cannot enter part details, and neither can CAD.
function editedFields(fields, currentFields) {
  if (fields.length !== currentFields.length) {
    fail(`fields must list the same ${currentFields.length} fields as now, in the same order`);
  }
  return fields.map((field, index) => {
    if (!field || field.name !== currentFields[index].name) {
      fail(`field ${index + 1} must be ${currentFields[index].name}; an edit cannot add, remove, rename or reorder fields`);
    }
    if (typeof field.value !== "string" || field.value.length > 10000) {
      fail(`the value of ${field.name} must be text of at most 10000 characters`);
    }
    return { name: field.name, value: field.value };
  });
}

// The checked manufacturer parts of a details section.
function editedManufacturerParts(parts) {
  if (parts.length > 100) {
    fail("manufacturerParts must be a list of at most 100");
  }
  const seen = {};
  return parts.map((part) => {
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
}

// ---- What part details and part revisions may hold -------------------------
//
// The import route and the edit route both check every details and
// revision document they save, so a field can never move between the two
// kinds: Symbol and Footprint are CAD, everything else describes the part.

// The fields of a part revision, in the order it keeps them.
const CAD_FIELDS = ["Symbol", "Footprint"];

function isPair(value, keys) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const names = Object.keys(value).sort();
  const wanted = keys.slice().sort();
  return (
    names.length === wanted.length &&
    names.every((name, index) => name === wanted[index] && typeof value[name] === "string")
  );
}

// Why `details` (parsed part details) cannot be saved, or "": its fields
// and manufacturer parts must be pairs of text, and no field may be
// Symbol or Footprint.
function detailsProblem(details) {
  const fields = details.fields;
  if (!Array.isArray(fields) || !fields.every((field) => isPair(field, ["name", "value"]))) {
    return "its fields must each be a name and a value, both text";
  }
  const cad = fields.find((field) => CAD_FIELDS.indexOf(field.name) >= 0);
  if (cad) {
    return `it lists ${cad.name}, which belongs to the part's revision, not its details`;
  }
  const parts = details.manufacturer_parts;
  if (!Array.isArray(parts) || !parts.every((part) => isPair(part, ["manufacturer", "mpn"]))) {
    return "its manufacturer parts must each be a manufacturer and an MPN, both text";
  }
  return "";
}

// The references a CAD field names, read the way the CSV import reads them:
// `;` separates them, each is trimmed like Rust's str::trim, blanks drop.
function namedReferences(text) {
  return text
    .split(";")
    .map((name) => trimSpace(name))
    .filter((name) => name.length > 0);
}

// Why `revision` (a parsed part revision) cannot be saved, or "": its CAD
// fields must be Symbol and Footprint only, each at most once and in that
// order, as pairs of text, and its CAD references must be exactly the ones
// those fields name, in order.
function revisionProblem(revision) {
  const fields = revision.cad_fields;
  if (!Array.isArray(fields) || !fields.every((field) => isPair(field, ["name", "value"]))) {
    return "its CAD fields must each be a name and a value, both text";
  }
  let position = -1;
  for (const field of fields) {
    const at = CAD_FIELDS.indexOf(field.name);
    if (at < 0) {
      return `it lists ${field.name}, which is part details, not CAD; a revision holds only Symbol and Footprint`;
    }
    if (at <= position) {
      return "its CAD fields must be Symbol, then Footprint, each at most once";
    }
    position = at;
  }
  const references = revision.cad_references;
  if (!Array.isArray(references)) {
    return "its CAD references must be a list";
  }
  const expected = [];
  for (const field of fields) {
    const role = field.name === "Symbol" ? "symbol" : "footprint";
    namedReferences(field.value).forEach((reference, ordinal) => {
      expected.push({ role: role, ordinal: ordinal, reference: reference });
    });
  }
  const matches =
    references.length === expected.length &&
    references.every(
      (item, index) =>
        item &&
        item.role === expected[index].role &&
        item.ordinal === expected[index].ordinal &&
        item.reference === expected[index].reference &&
        item.target &&
        typeof item.target === "object",
    );
  if (!matches) {
    return "its CAD references are not the ones its Symbol and Footprint fields name";
  }
  return "";
}

// ---- Archived revisions -------------------------------------------------------
//
// A part revision is archived when its newest catalog_archive_entries
// record (by Change seq) says so. The part's current revision is its newest
// revision that is not archived.

function recordById(txApp, collection, id) {
  try {
    return txApp.findRecordById(collection, id);
  } catch (_) {
    return null;
  }
}

// Each revision number's archive state, from the part's archive entries:
// { number: true } for archived, false for unarchived; numbers with no
// entry are not archived.
function archiveState(txApp, partId) {
  const state = {};
  const entries = txApp.findRecordsByFilter("catalog_archive_entries", "part = {:part}", "seq", 0, 0, {
    part: partId,
  });
  for (const entry of entries) {
    state[entry.getInt("revision_number")] = entry.getBool("archived");
  }
  return state;
}

// The part's revisions, newest first.
function revisionsNewestFirst(txApp, partId) {
  return txApp.findRecordsByFilter("catalog_part_revisions", "part = {:part}", "-number", 0, 0, { part: partId });
}

// The part's newest revision, archived or not, or null.
function newestRevision(txApp, partId) {
  const found = txApp.findRecordsByFilter("catalog_part_revisions", "part = {:part}", "-number", 1, 0, {
    part: partId,
  });
  return found.length > 0 ? found[0] : null;
}

// The part's newest revision that `state` (from archiveState) does not mark
// archived, or null when every revision is archived.
function newestNotArchived(txApp, partId, state) {
  for (const revision of revisionsNewestFirst(txApp, partId)) {
    if (state[revision.getInt("number")] !== true) {
      return revision;
    }
  }
  return null;
}

// A hash of a CAD item revision's content: its snapshot with the numbers of
// its 3D model bindings left out, over key-sorted JSON (see the import
// route). Never compare it with crates/catalog's content hash.
function cadContent(snapshot) {
  const parsed = JSON.parse(snapshot);
  for (const model of parsed.model_references || []) {
    if (model && model.target && model.target.kind === "cad-item-revision") {
      model.target.number = 0;
    }
  }
  return $security.sha256(canonicalJson(parsed));
}

// A hash of a part revision's CAD content: its document with every bound
// CAD item revision replaced by that revision's content, so the same CAD
// under newer CAD item numbers hashes the same. `contentFor(target)` gives
// a bound CAD item revision's content.
function partRevisionContent(parsed, contentFor) {
  const copy = JSON.parse(JSON.stringify(parsed));
  for (const reference of copy.cad_references || []) {
    const target = reference && reference.target;
    if (target && target.kind === "cad-item-revision") {
      reference.target = { kind: target.kind, cad_item: target.cad_item, content: contentFor(target) };
    }
  }
  delete copy.excluded_from_bom;
  return $security.sha256(canonicalJson(copy));
}

// contentFor for stored CAD item revisions.
function storedCadContent(txApp) {
  return (target) => {
    const record = recordById(
      txApp,
      "catalog_cad_item_revisions",
      deriveId("cad-item-revision", target.cad_item + "\n" + target.number),
    );
    return record ? cadContent(record.getString("snapshot")) : "missing:" + target.cad_item + ":" + target.number;
  };
}

// The number of the part's archived revision whose CAD content is
// `content` (from partRevisionContent), or 0.
function archivedRevisionWithContent(txApp, partId, content) {
  const state = archiveState(txApp, partId);
  const stored = storedCadContent(txApp);
  for (const revision of revisionsNewestFirst(txApp, partId)) {
    const number = revision.getInt("number");
    if (state[number] === true && partRevisionContent(JSON.parse(revision.getString("snapshot")), stored) === content) {
      return number;
    }
  }
  return 0;
}

// ---- CAD item revisions ----------------------------------------------------
//
// CAD item revisions are numbered from 1 per CAD item, like part revisions,
// and the current revision is the newest. Part details and footprint
// snapshots name one as { kind: "cad-item-revision", cad_item, number,
// snapshot_sha256 }: the CAD item and number give its ID, and the hash lets
// a reader check the record it finds.

// A CAD binding read from a client's snapshot, checked for shape: exactly
// those four keys, a CAD item ID, a number from 1 and a SHA-256. `who` names
// the binding in error messages. Returns { cadItem, number,
// snapshotSha256, id, who }; throws BadRequestError.
function cadItemRevisionTarget(target, who) {
  const keys = Object.keys(target).sort().join(",");
  if (keys !== "cad_item,kind,number,snapshot_sha256") {
    throw new BadRequestError(`${who} must name a CAD item revision by cad_item, number and snapshot_sha256 only`);
  }
  const cadItem = target.cad_item;
  const number = target.number;
  const snapshotSha256 = target.snapshot_sha256;
  if (typeof cadItem !== "string" || !/^[0-9a-f]{15}$/.test(cadItem)) {
    throw new BadRequestError(`${who} names a CAD item that is not a CAD item ID`);
  }
  if (!Number.isInteger(number) || number < 1) {
    throw new BadRequestError(`${who} names a revision number that is not a whole number from 1`);
  }
  if (typeof snapshotSha256 !== "string" || !/^[0-9a-f]{64}$/.test(snapshotSha256)) {
    throw new BadRequestError(`${who} names a snapshot hash that is not 64 lowercase hex digits`);
  }
  return {
    cadItem: cadItem,
    number: number,
    snapshotSha256: snapshotSha256,
    id: deriveId("cad-item-revision", cadItem + "\n" + number),
    who: who,
  };
}

// Whether a stored record really is revision `number` of CAD item
// `cadItemId`: its ID is the derived one and its snapshot matches its hash.
function isCadItemRevision(record, cadItemId, number) {
  return (
    record.id === deriveId("cad-item-revision", cadItemId + "\n" + number) &&
    record.getString("cad_item") === cadItemId &&
    record.getInt("number") === number &&
    $security.sha256(record.getString("snapshot")) === record.getString("snapshot_sha256")
  );
}

// The current (newest) revision of a CAD item, or null if it has none.
// Throws if the stored record is not what its ID says.
function currentCadItemRevision(txApp, cadItemId) {
  const found = txApp.findRecordsByFilter("catalog_cad_item_revisions", "cad_item = {:id}", "-number", 1, 0, {
    id: cadItemId,
  });
  if (found.length === 0) {
    return null;
  }
  const current = found[0];
  if (!isCadItemRevision(current, cadItemId, current.getInt("number"))) {
    throw new Error(`the stored record ${current.id} is not revision ${current.getInt("number")} of CAD item ${cadItemId}`);
  }
  return current;
}

// The binding to `revision`, with keys in the order Rust writes them.
function bindingTo(cadItemId, revision) {
  return {
    kind: "cad-item-revision",
    cad_item: cadItemId,
    number: revision.getInt("number"),
    snapshot_sha256: revision.getString("snapshot_sha256"),
  };
}

// The CAD references of a new revision, read from its Symbol and Footprint
// fields the way the CSV import reads them (`;` separates footprint
// choices). Each reference to the CSV catalog's own libraries
// (`g-*`) is bound to one exact CAD item revision:
// - a reference the current revision already binds keeps its exact
//   revision, so editing another field never moves a CAD binding (the same
//   content under a newer number is not a change);
// - any other reference binds the current revision of the CAD item with
//   exactly that library reference;
// - a reference that the current revision kept as unresolved stays so;
// - anything else is refused: an edit can only use CAD items the Catalog
//   already has.
// Other libraries (such as KiCad's own) are external, as in the import.
function cadReferences(txApp, fields, currentRevision) {
  const value = (name) => {
    const field = fields.find((item) => item.name === name);
    return field ? field.value : null;
  };
  const previous = currentRevision.cad_references || [];
  const references = [];
  for (const [role, column] of [["symbol", "Symbol"], ["footprint", "Footprint"]]) {
    const text = value(column);
    if (text === null) {
      continue;
    }
    const names = namedReferences(text);
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
          target = {
            kind: "cad-item-revision",
            cad_item: cadItemId,
            number: same.target.number,
            snapshot_sha256: same.target.snapshot_sha256,
          };
        } else if (cadItem) {
          const current = currentCadItemRevision(txApp, cadItemId);
          if (!current) {
            throw new BadRequestError(`the ${role} ${reference} has no revision`);
          }
          target = bindingTo(cadItemId, current);
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

// The CAD item revision IDs a revision binds, for the relation field.
function boundCadItemRevisions(details) {
  const bound = [];
  for (const reference of details.cad_references || []) {
    if (reference.target && reference.target.kind === "cad-item-revision") {
      bound.push(deriveId("cad-item-revision", reference.target.cad_item + "\n" + reference.target.number));
    }
  }
  return bound;
}

// What a client needs to show part details or a revision someone else
// saved.
function describeRecord(txApp, revision) {
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

function publishEdit(e) {
  const kind = CHANGE_KINDS.edit;
  const actor = catalogEditor(e);
  const request = editRequest(e.requestInfo().body || {});
  // The request's identity, from validated values in a fixed order (the
  // parsed body is a Go map, whose key order varies).
  const pairs = (fields) => fields.map((field) => [field && field.name, field && field.value]);
  const requestHash = $security.sha256(
    JSON.stringify([
      kind,
      request.ipn,
      request.reason,
      request.details
        ? [
            request.details.expected,
            pairs(request.details.fields),
            request.details.manufacturerParts.map((part) => [part && part.manufacturer, part && part.mpn]),
          ]
        : null,
      request.cad ? [request.cad.expected, pairs(request.cad.fields)] : null,
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
    const currentDetails = byId("catalog_part_details", part.getString("current_details"));
    const currentRevision = byId("catalog_part_revisions", part.getString("current_revision"));
    if (!currentDetails || !currentRevision) {
      throw new Error(`${request.ipn} has no current details or revision`);
    }
    const detailsMoved = request.details && request.details.expected !== currentDetails.getInt("number");
    const revisionMoved = request.cad && request.cad.expected !== currentRevision.getInt("number");
    if (detailsMoved || revisionMoved) {
      const what = [];
      if (detailsMoved) {
        what.push(`its part details are now number ${currentDetails.getInt("number")}, not ${request.details.expected}`);
      }
      if (revisionMoved) {
        what.push(`its current revision is ${currentRevision.getInt("number")}, not ${request.cad.expected}`);
      }
      const body = {
        outcome: "conflict",
        correlationId: request.correlationId,
        ipn: request.ipn,
        message: `someone else changed ${request.ipn} first: ${what.join(", and ")}`,
      };
      if (detailsMoved) {
        body.currentDetails = describeRecord(txApp, currentDetails);
      }
      if (revisionMoved) {
        body.currentRevision = describeRecord(txApp, currentRevision);
      }
      reply = { status: 409, body: body };
      return;
    }
    const storedDetails = JSON.parse(currentDetails.getString("snapshot"));
    if (storedDetails.schema !== PART_DETAILS_SCHEMA || storedDetails.ipn !== request.ipn) {
      throw new Error(`the current details of ${request.ipn} are not ${PART_DETAILS_SCHEMA} part details`);
    }
    const storedRevision = JSON.parse(currentRevision.getString("snapshot"));
    if (storedRevision.schema !== PART_REVISION_SCHEMA || storedRevision.ipn !== request.ipn) {
      throw new Error(`the current revision of ${request.ipn} is not a ${PART_REVISION_SCHEMA} part revision`);
    }

    // The new documents, serialized in the field order of crates/catalog
    // PartDetails and PartRevision, so the stored bytes are their
    // canonical JSON.
    // Each new document is checked like an imported one, so a field can
    // never move between the details and the revision, even if a stored
    // document were wrong.
    const refuse = (what, problem) => {
      if (problem) {
        throw new BadRequestError(`the new ${what} of ${request.ipn} cannot be saved: ${problem}`);
      }
    };
    let details = null;
    if (request.details) {
      const document = {
        schema: PART_DETAILS_SCHEMA,
        ipn: request.ipn,
        fields: editedFields(request.details.fields, storedDetails.fields || []),
        manufacturer_parts: editedManufacturerParts(request.details.manufacturerParts),
      };
      refuse("details", detailsProblem(document));
      const snapshot = JSON.stringify(document);
      const sha = $security.sha256(snapshot);
      if (sha !== currentDetails.getString("snapshot_sha256")) {
        details = { snapshot: snapshot, sha: sha };
      }
    }
    let revision = null;
    if (request.cad) {
      const fields = editedFields(request.cad.fields, storedRevision.cad_fields || []);
      const document = {
        schema: PART_REVISION_SCHEMA,
        ipn: request.ipn,
        cad_fields: fields,
        cad_references: cadReferences(txApp, fields, storedRevision),
      };
      // An edit keeps Excluded from BOM as it is (kicad-parts-ccc0 decides
      // how it follows new CAD). Like PartRevision, the snapshot names it
      // only when it is set.
      if (storedRevision.excluded_from_bom === true) {
        document.excluded_from_bom = true;
      }
      refuse("revision", revisionProblem(document));
      const snapshot = JSON.stringify(document);
      const sha = $security.sha256(snapshot);
      // Only CAD that really changes can bring an archived design back; an
      // unchanged CAD section (sent with a details edit) never does.
      if (sha !== currentRevision.getString("snapshot_sha256")) {
        const archived = archivedRevisionWithContent(
          txApp,
          partId,
          partRevisionContent(document, storedCadContent(txApp)),
        );
        if (archived > 0) {
          throw new BadRequestError(
            `this symbol and footprint are those of archived revision ${archived} of ${request.ipn}; unarchive revision ${archived} instead of saving them again`,
          );
        }
        revision = { snapshot: snapshot, sha: sha };
      }
    }

    let status;
    let result;
    if (!details && !revision) {
      status = 422;
      result = {
        outcome: "noop",
        correlationId: request.correlationId,
        ipn: request.ipn,
        message: "the edit changes nothing",
      };
    } else {
      const record = (name, id) => {
        const made = new Record(txApp.findCollectionByNameOrId(name));
        made.set("id", id);
        return made;
      };
      const last = first("catalog_changes", "seq > 0", {}, "-seq");
      const seq = last ? last.getInt("seq") + 1 : 1;
      const changeId = deriveId("change", request.correlationId);
      const source = (previous) =>
        JSON.stringify({
          schema: SOURCE_SCHEMA,
          source: CHANGE_KINDS.edit,
          previous: { id: previous.id, number: previous.getInt("number") },
        });
      const detailsNumber = details ? currentDetails.getInt("number") + 1 : null;
      // A new revision comes after the newest one, which may be archived.
      const newest = newestRevision(txApp, partId);
      const revisionNumber = revision ? newest.getInt("number") + 1 : null;
      const summary = JSON.stringify({
        ipn: request.ipn,
        partDetails: details ? 1 : 0,
        partRevisions: revision ? 1 : 0,
        details: detailsNumber,
        revision: revisionNumber,
      });

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

      let savedDetails = null;
      if (details) {
        const id = deriveId("part-details", partId + "\n" + detailsNumber);
        const made = record("catalog_part_details", id);
        made.set("part", partId);
        made.set("number", detailsNumber);
        made.set("predecessor", currentDetails.id);
        made.set("change", changeId);
        made.set("snapshot", details.snapshot);
        made.set("snapshot_sha256", details.sha);
        made.set("source", source(currentDetails));
        txApp.save(made);
        part.set("current_details", id);
        part.set("current_details_number", detailsNumber);
        savedDetails = { id: id, number: detailsNumber, snapshotSha256: details.sha };
      }
      let savedRevision = null;
      // The CAD item revisions the part's current revision binds, as plain
      // JavaScript strings (the stored list is a Go slice).
      const stored = currentRevision.getStringSlice("cad_item_revisions");
      let bound = [];
      for (let i = 0; i < stored.length; i++) {
        bound.push(String(stored[i]));
      }
      if (revision) {
        const id = deriveId("part-revision", partId + "\n" + revisionNumber);
        bound = boundCadItemRevisions(JSON.parse(revision.snapshot));
        const made = record("catalog_part_revisions", id);
        made.set("part", partId);
        made.set("number", revisionNumber);
        made.set("predecessor", newest.id);
        made.set("change", changeId);
        made.set("snapshot", revision.snapshot);
        made.set("snapshot_sha256", revision.sha);
        made.set("source", source(currentRevision));
        made.set("cad_item_revisions", bound);
        txApp.save(made);
        part.set("current_revision", id);
        part.set("current_revision_number", revisionNumber);
        savedRevision = { id: id, number: revisionNumber, snapshotSha256: revision.sha };
      }
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
        details: savedDetails,
        revision: savedRevision,
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

// ---- Archive and unarchive --------------------------------------------------
//
// POST /api/catalog/archives (see catalog_archives.pb.js) archives or
// unarchives one part revision, in one transaction: replay a repeated
// correlation ID; 404 if the part has no such revision; 422 noop if the
// revision already has that state; 400 if archiving would leave no revision
// that is not archived; otherwise save one Change, one archive entry, the
// part's move to its newest revision that is not archived (if that
// changed), and one audit entry. Nothing is deleted.
function publishArchive(e) {
  const actor = catalogEditor(e);
  const body = e.requestInfo().body || {};
  const request = commonRequest(body);
  const revision = body.revision;
  if (typeof revision !== "number" || !Number.isInteger(revision) || revision < 1) {
    fail("revision must be the number of one of the part's revisions");
  }
  const archived = body.archived;
  if (typeof archived !== "boolean") {
    fail("archived must be true (archive) or false (unarchive)");
  }
  const kind = archived ? CHANGE_KINDS.archive : CHANGE_KINDS.unarchive;
  const requestHash = $security.sha256(JSON.stringify([kind, request.ipn, request.reason, revision]));

  let reply = null;
  $app.runInTransaction((txApp) => {
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
    const part = recordById(txApp, "catalog_parts", partId);
    if (!part || part.getString("ipn") !== request.ipn) {
      throw new NotFoundError(`${request.ipn} is not a part in the Catalog`);
    }
    const target = recordById(txApp, "catalog_part_revisions", deriveId("part-revision", partId + "\n" + revision));
    if (!target || target.getString("part") !== partId || target.getInt("number") !== revision) {
      throw new NotFoundError(`${request.ipn} has no revision ${revision}`);
    }
    const state = archiveState(txApp, partId);
    let status;
    let result;
    if ((state[revision] === true) === archived) {
      status = 422;
      result = {
        outcome: "noop",
        correlationId: request.correlationId,
        ipn: request.ipn,
        message: archived ? `revision ${revision} is already archived` : `revision ${revision} is not archived`,
      };
    } else {
      state[revision] = archived;
      const current = newestNotArchived(txApp, partId, state);
      if (!current) {
        throw new BadRequestError(
          `revision ${revision} is the only revision that is not archived, so it cannot be archived. Add a corrected revision first`,
        );
      }
      const record = (name, id) => {
        const made = new Record(txApp.findCollectionByNameOrId(name));
        made.set("id", id);
        return made;
      };
      const last = first("catalog_changes", "seq > 0", {}, "-seq");
      const seq = last ? last.getInt("seq") + 1 : 1;
      const changeId = deriveId("change", request.correlationId);
      const summary = JSON.stringify({
        ipn: request.ipn,
        revision: revision,
        archived: archived,
        currentRevision: current.getInt("number"),
      });
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

      const entryId = deriveId("archive-entry", request.correlationId);
      const entry = record("catalog_archive_entries", entryId);
      entry.set("part", partId);
      entry.set("part_revision", target.id);
      entry.set("revision_number", revision);
      entry.set("archived", archived);
      entry.set("change", changeId);
      entry.set("seq", seq);
      entry.set("current_revision_after", current.id);
      entry.set("current_revision_number_after", current.getInt("number"));
      // Saving the entry moves the part's current revision to `current`
      // (catalog_write_guards.pb.js), so the route does not save the part.
      txApp.save(entry);

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
        revision: revision,
        archived: archived,
        currentRevision: {
          id: current.id,
          number: current.getInt("number"),
          snapshotSha256: current.getString("snapshot_sha256"),
        },
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
  PART_REVISION_SCHEMA: PART_REVISION_SCHEMA,
  SOURCE_SCHEMA: SOURCE_SCHEMA,
  publishEdit: publishEdit,
  publishArchive: publishArchive,
  archiveState: archiveState,
  newestRevision: newestRevision,
  newestNotArchived: newestNotArchived,
  cadContent: cadContent,
  partRevisionContent: partRevisionContent,
  storedCadContent: storedCadContent,
  archivedRevisionWithContent: archivedRevisionWithContent,
  detailsProblem: detailsProblem,
  revisionProblem: revisionProblem,
  describeRecord: describeRecord,
  cadItemRevisionTarget: cadItemRevisionTarget,
  isCadItemRevision: isCadItemRevision,
  trimSpace: trimSpace,
  isReservedName: isReservedName,
  hasInvisible: hasInvisible,
  guardedCollection: guardedCollection,
  collectionChangeProblem: collectionChangeProblem,
  canonicalJson: canonicalJson,
  deriveId: deriveId,
  sha256Bytes: sha256Bytes,
  base64Decode: base64Decode,
};

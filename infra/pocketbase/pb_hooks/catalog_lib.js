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
  csvReview: "csv-review",
  create: "create",
});

// The `source` value of rows read from the CSV catalog.
const CSV_CATALOG_SOURCE = "csv-catalog";

// Schema tags shared with crates/catalog (PART_DETAILS_SCHEMA,
// PART_REVISION_SCHEMA and SOURCE_SCHEMA); a test there keeps them in step.
const PART_DETAILS_SCHEMA = "kicad-parts/part-details/3";
const PART_REVISION_SCHEMA = "kicad-parts/part-revision/1";
const SOURCE_SCHEMA = "kicad-parts/source/1";
// The schema tag of a category revision's snapshot, shared with
// crates/catalog (CATEGORY_SCHEMA); a test there keeps it in step.
const CATEGORY_SCHEMA = "kicad-parts/category/1";

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
    const category = details.category === undefined || details.category === null ? null : details.category;
    if (category !== null && (typeof category !== "string" || !CATEGORY_CODE.test(category))) {
      fail("details.category must be the code of a category, such as res");
    }
    details = {
      expected: number(details, "expectedDetails", "details"),
      category: category,
      fields: details.fields,
      manufacturerParts: details.manufacturerParts,
    };
  }
  if (cad) {
    if (!Array.isArray(cad.fields)) {
      fail("cad needs fields");
    }
    cad = {
      expected: number(cad, "expectedRevision", "revision"),
      fields: cad.fields,
      uploads: editUploads(cad.uploads),
    };
  }
  return { correlationId: correlationId, reason: reason, ipn: ipn, details: details, cad: cad };
}

// ---- Files uploaded with an edit -------------------------------------------
//
// The cad section of an edit may carry files (kicad-parts-3b8b): a symbol,
// a footprint or a 3D model, each with the reference it is for and its
// bytes, sent as text or base64 exactly like the blobs of a CSV import.
// The server reads every file itself: a symbol must be one top-level
// (symbol "NAME" ...) whose NAME is the reference's name, a footprint a
// whole .kicad_mod whose (footprint "NAME" ...) is, and a 3D model any
// bytes that a footprint uploaded with it names. It hashes the bytes, and
// saves blobs and CAD item revisions in the shape, numbering and IDs of the
// import (catalog_csv_imports.pb.js).

// Each upload role and the CAD item kind it makes.
const UPLOAD_KINDS = Object.freeze({ symbol: "symbol", footprint: "footprint", "3d-model": "model-3d" });

// How the bytes of each kind are read, as crates/catalog CadItemDetails
// names it.
const CAD_FORMATS = Object.freeze({
  symbol: "kicad-symbol-entry",
  footprint: "kicad-footprint",
  "model-3d": "3d-model",
});

const CAD_ITEM_DETAILS_SCHEMA = "kicad-parts/cad-item-details/1";

// The license every CAD item revision without stated terms records.
const UNKNOWN_LICENSE = "unknown-needs-review";

// The `source` of CAD item revisions an edit uploads.
const UPLOAD_SOURCE = "edit";

// The `source` of CAD item revisions read from the CSV catalog's own
// libraries (Add part from our libraries, kicad-parts-37a7). Like a CSV
// import's, it names the repository file, the commit it was read from, the
// whole file's SHA-256 and, for a symbol, its byte range in the file.
const LIBRARY_SOURCE = "library";

// Where an upload says it was read from in our libraries, checked against
// the upload itself: { path, byteSha256, commit, byteRange }, or null when
// it says nothing. A symbol is in symbols/<library>.kicad_sym with its
// byte range; a footprint is the whole file
// footprints/<library>.pretty/<name>.kicad_mod; a 3D model the whole file
// at its own path. The commit is what the app says it read the file from:
// the server has no repository, so it checks the commit is written as a
// full ID, not that it holds the file; the bytes it stores are checked as
// for any upload.
function uploadLibrary(upload, which, role, reference, bytes, sha256) {
  const library = upload.library;
  if (library === undefined || library === null) {
    return null;
  }
  if (typeof library !== "object" || Array.isArray(library)) {
    fail(`${which} must say where it is in our libraries as an object`);
  }
  const colon = reference.indexOf(":");
  const expected =
    role === "symbol"
      ? `symbols/${reference.substring(0, colon)}.kicad_sym`
      : role === "footprint"
        ? `footprints/${reference.substring(0, colon)}.pretty/${reference.substring(colon + 1)}.kicad_mod`
        : reference;
  if (library.path !== expected) {
    fail(`the ${role} ${reference} is in our libraries at ${expected}, not ${String(library.path)}`);
  }
  if (typeof library.commit !== "string" || !/^([0-9a-f]{40}|[0-9a-f]{64})$/.test(library.commit)) {
    fail(`the ${role} ${reference} needs the commit it was read from, as its full ID`);
  }
  if (typeof library.byteSha256 !== "string" || !/^[0-9a-f]{64}$/.test(library.byteSha256)) {
    fail(`the ${role} ${reference} needs the SHA-256 of ${expected}`);
  }
  let byteRange = null;
  if (role === "symbol") {
    const range = library.byteRange;
    const whole = (value) => typeof value === "number" && Number.isInteger(value) && value >= 0;
    if (!Array.isArray(range) || range.length !== 2 || !whole(range[0]) || !whole(range[1]) || range[1] - range[0] !== bytes.length) {
      fail(`the symbol ${reference} needs its byte range in ${expected}, as long as the symbol`);
    }
    byteRange = [range[0], range[1]];
  } else {
    if (library.byteRange !== undefined && library.byteRange !== null) {
      fail(`only a symbol has a byte range, not the ${role} ${reference}`);
    }
    if (library.byteSha256 !== sha256) {
      fail(`the ${role} ${reference} is not the whole file ${expected}`);
    }
  }
  return { path: expected, byteSha256: library.byteSha256, commit: library.commit, byteRange: byteRange };
}

// The `source` of CAD item revisions copied from another of the person's
// installed KiCad libraries (kicad-parts-bc6f): not ours and not KiCad's
// own, so teammates may not have it. Like a download, it is CAD made in the
// app, so a later CSV import never counts it as imported. Its Source names
// the library's nickname and the file it was read from.
const INSTALLED_SOURCE = "installed";

// Where an upload says it was copied from in an installed library:
// { library, path }, or null when it says nothing. Only the words are
// checked (the server cannot see the person's computer); the bytes it
// stores are checked as for any upload.
function uploadInstalled(upload, which, role, reference) {
  const installed = upload.installed;
  if (installed === undefined || installed === null) {
    return null;
  }
  if (typeof installed !== "object" || Array.isArray(installed)) {
    fail(`${which} must say where it was copied from as an object`);
  }
  if (upload.library !== undefined && upload.library !== null) {
    fail(`the ${role} ${reference} cannot come both from our libraries and from an installed library`);
  }
  const plain = (text, longest) =>
    typeof text === "string" && trimSpace(text).length > 0 && text.length <= longest && !CONTROL_CHARACTER.test(text);
  if (!plain(installed.library, 255)) {
    fail(`the ${role} ${reference} needs the nickname of the installed library it was copied from, of at most 255 characters`);
  }
  if (!plain(installed.path, 1000)) {
    fail(`the ${role} ${reference} needs the file it was copied from, of at most 1000 characters`);
  }
  return { library: installed.library, path: installed.path };
}

// What the Source of a revision read from our libraries holds, for
// filters: CSV imports treat such revisions as imported, not made in the app.
const LIBRARY_SOURCE_MARK = '"source":"library"';

// Whether a stored CAD item revision was read from our libraries.
function isFromOurLibraries(revision) {
  try {
    return JSON.parse(revision.getString("source")).source === LIBRARY_SOURCE;
  } catch (error) {
    return false;
  }
}

// What a request's hash records of an upload.
function hashedUpload(upload) {
  const hashed = [upload.role, upload.reference, upload.fileName, upload.formatVersion, upload.sha256];
  if (upload.library) {
    hashed.push([upload.library.path, upload.library.byteSha256, upload.library.commit, upload.library.byteRange]);
  }
  if (upload.installed) {
    hashed.push(["installed", upload.installed.library, upload.installed.path]);
  }
  return hashed;
}

// The checked uploads of a cad section: role, CAD item kind, reference,
// file name, KiCad format version (symbols only) and bytes with their
// SHA-256, each hashed here.
function editUploads(list) {
  if (list === undefined || list === null) {
    return [];
  }
  if (!Array.isArray(list)) {
    fail("cad uploads must be a list");
  }
  if (list.length > 20) {
    fail("an edit can upload at most 20 files");
  }
  const uploads = list.map((upload, index) => {
    const which = `upload ${index + 1}`;
    if (!upload || typeof upload !== "object" || Array.isArray(upload)) {
      fail(`${which} must be an object`);
    }
    const role = upload.role;
    if (typeof role !== "string" || !Object.prototype.hasOwnProperty.call(UPLOAD_KINDS, role)) {
      fail(`${which} has the role ${String(role)}; the role must be symbol, footprint or 3d-model`);
    }
    const reference = upload.reference;
    if (typeof reference !== "string" || reference.length === 0 || reference.length > 500) {
      fail(`${which} needs the reference it is for, of at most 500 characters`);
    }
    const fileName = upload.fileName;
    if (typeof fileName !== "string" || trimSpace(fileName).length === 0 || fileName.length > 255) {
      fail(`${which} needs the name of the file it came from, of at most 255 characters`);
    }
    // Only KiCad's 3D model files (crates/catalog cad_file
    // MODEL_EXTENSIONS), so no other file, such as a key, is ever stored.
    if (role === "3d-model" && !/\.(step|stp|stpz|wrl|wrz)$/i.test(reference)) {
      fail(`the 3D model ${reference} must be a .step, .stp, .stpz, .wrl or .wrz file`);
    }
    let formatVersion = null;
    if (role === "symbol") {
      formatVersion = upload.formatVersion;
      if (typeof formatVersion !== "string" || !/^20[0-9]{6}$/.test(formatVersion)) {
        fail(`the symbol ${reference} needs the (version ...) of the KiCad file it came from`);
      }
    } else if (upload.formatVersion !== undefined && upload.formatVersion !== null) {
      fail(`only a symbol has a format version, not the ${role} ${reference}`);
    }
    const hasText = upload.text !== undefined && upload.text !== null;
    const hasBase64 = upload.base64 !== undefined && upload.base64 !== null;
    if (hasText === hasBase64) {
      fail(`the file for the ${role} ${reference} must be sent as either text or base64`);
    }
    let bytes;
    let sha256;
    if (hasText) {
      if (typeof upload.text !== "string") {
        fail(`the text of the file for the ${role} ${reference} must be a string`);
      }
      bytes = toBytes(upload.text);
      sha256 = $security.sha256(upload.text);
    } else {
      if (role !== "3d-model") {
        // KiCad saves symbols and footprints as UTF-8 text.
        fail(`the file for the ${role} ${reference} must be text, as KiCad saves it`);
      }
      bytes = base64Decode(upload.base64);
      if (bytes === null) {
        fail(`the file for the ${role} ${reference} is not base64`);
      }
      sha256 = sha256Bytes(bytes);
    }
    if (bytes.length === 0) {
      fail(`the file for the ${role} ${reference} is empty`);
    }
    return {
      role: role,
      kind: UPLOAD_KINDS[role],
      reference: reference,
      fileName: fileName,
      formatVersion: formatVersion,
      text: hasText ? upload.text : null,
      bytes: bytes,
      sha256: sha256,
      library: uploadLibrary(upload, which, role, reference, bytes, sha256),
      installed: uploadInstalled(upload, which, role, reference),
    };
  });
  const seen = {};
  for (const upload of uploads) {
    const key = upload.kind + "\n" + upload.reference;
    if (seen[key]) {
      fail(`two files were sent for the ${upload.role} ${upload.reference}; send one`);
    }
    seen[key] = true;
  }
  return uploads;
}

// The 3D model references of a footprint, in file order, exactly as
// crates/kicad-integration library_file.rs model_references (and the CSV
// catalog fixture) extract them from the file's text.
function modelReferences(content) {
  const paths = [];
  const pieces = content.split("(model");
  for (let i = 1; i < pieces.length; i++) {
    const value = pieces[i].replace(LEADING_SPACE, "");
    if (value.charAt(0) !== '"') {
      continue;
    }
    let escaped = false;
    let output = "";
    for (const ch of value.substring(1)) {
      if (!escaped && ch === '"') {
        break;
      }
      escaped = !escaped && ch === "\\";
      output += ch;
      if (ch !== "\\") {
        escaped = false;
      }
    }
    paths.push(output);
  }
  return paths;
}

// `relative` as a clean `/`-separated path, or null if it could escape,
// as crates/catalog csv_import safe_relative.
function safeRelative(relative) {
  const drive = relative.length >= 2 && /^[A-Za-z]$/.test(relative.charAt(0)) && relative.charAt(1) === ":";
  if (relative.length === 0 || relative.charAt(0) === "/" || relative.charAt(0) === "\\" || drive) {
    return null;
  }
  const parts = [];
  for (const component of relative.split(/[/\\]/)) {
    if (component === "" || component === ".") {
      continue;
    }
    if (component === ".." || component.indexOf(":") >= 0) {
      return null;
    }
    parts.push(component);
  }
  return parts.length > 0 ? parts.join("/") : null;
}

// The lexical classification of a 3D model reference, as crates/catalog
// csv_import classify_model_reference: { scope, status, candidate }, where
// candidate is the repository path under 3d-models/ for a local reference.
function classifyModelReference(reference) {
  const strip = (variable) => {
    if (!reference.startsWith(variable)) {
      return null;
    }
    const rest = reference.substring(variable.length);
    return rest.charAt(0) === "/" || rest.charAt(0) === "\\" ? rest.substring(1) : null;
  };
  const local = (relative) => {
    const clean = safeRelative(relative);
    return clean === null
      ? { scope: "local", status: "unsafe-path", candidate: null }
      : { scope: "local", status: "missing", candidate: "3d-models/" + clean };
  };
  const parts = strip("${GITPLM_PARTS}");
  if (parts !== null) {
    const separator = parts.charAt("3d-models".length);
    if (parts.startsWith("3d-models") && (separator === "/" || separator === "\\")) {
      return local(parts.substring("3d-models".length + 1));
    }
    return { scope: "local", status: "unsafe-path", candidate: null };
  }
  const models = strip("${GITPLM_3DMODELS}");
  if (models !== null) {
    return local(models);
  }
  const drive = reference.length >= 2 && /^[A-Za-z]$/.test(reference.charAt(0)) && reference.charAt(1) === ":";
  let scope;
  if (reference.startsWith("\\\\") || reference.startsWith("//")) {
    scope = "unc-absolute";
  } else if (drive && (reference.charAt(2) === "/" || reference.charAt(2) === "\\")) {
    scope = "windows-drive-absolute";
  } else if (reference.startsWith("/")) {
    scope = "posix-absolute";
  } else if (reference.startsWith("\\")) {
    scope = "windows-rooted";
  } else if (drive) {
    scope = "windows-drive-relative";
  } else if (reference.startsWith("${")) {
    scope = "external-variable";
  } else {
    scope = "relative";
  }
  return { scope: scope, status: "unresolved", candidate: null };
}

// Characters Rust's char::is_control matches (Unicode category Cc).
const CONTROL_CHARACTER = /[\u0000-\u001F\u007F-\u009F]/;

// Whether `name` can be one half of LIB:NAME: the rule crates/kicad-integration
// LibRef::parse applies (not empty, . or .., and no / \\ : or NUL), with no
// other control character and no leading "-", as crates/catalog cad_file
// is_plain_name. Both halves become file names, in KiCad's libraries and in
// preview folders, and a name can come from a downloaded file.
function isPlainName(name) {
  return (
    name.length > 0 &&
    name !== "." &&
    name !== ".." &&
    name.charAt(0) !== "-" &&
    !/[/\\:]/.test(name) &&
    !CONTROL_CHARACTER.test(name)
  );
}

// Why `reference` cannot take a file, or "": it must be LIB:NAME exactly as
// written (one colon, no spaces around it), both halves plain names, in one
// of the Catalog's own libraries (g- and more).
function uploadReferenceProblem(reference) {
  const parts = reference.split(":");
  if (parts.length !== 2 || trimSpace(reference) !== reference || !isPlainName(parts[0]) || !isPlainName(parts[1])) {
    return `${reference} cannot be used as a reference: write LIBRARY:NAME with one colon, no spaces around it, and a library and name that are not . or .., do not start with -, and hold no / \\ : or control characters`;
  }
  if (!parts[0].startsWith("g-") || parts[0].length < 3) {
    return `a file can only be added to the Catalog's own libraries, whose names start with g- followed by a name, such as g-res, not ${parts[0]}`;
  }
  return "";
}

// Whether `key` is where the Catalog keeps a 3D model: its footprint
// library's folder, 3d-models/g-<library>.3dshapes/<file>, with plain names
// (kicad-parts-82ea), so a model path can never leave that folder.
function isCatalogModelKey(key) {
  const match = /^3d-models\/([^/\\]+)\.3dshapes\/([^/\\]+)$/.exec(key);
  return (
    match !== null &&
    match[1].startsWith("g-") &&
    match[1].length > 2 &&
    isPlainName(match[1]) &&
    isPlainName(match[2])
  );
}

// The library check's rule for the 3D model links of a footprint file an
// edit or a new part sends (kicad-parts-da65, crates/catalog library_check
// judge_model_link and chosen_footprint_problems): KiCad's own models
// (${KICAD<n>_3DMODEL_DIR}/..., never with a .. part) and models embedded
// in the footprint (kicad-embed://...) are fine; a ${GITPLM_PARTS}/3d-models
// path must be a Catalog 3D model path that the Catalog has or the request
// sends (`available`); anything else only works on one computer. Gives why
// the footprint `reference` cannot be saved, or "". Imports are checked by
// the import tool, not here.
function modelLinkProblem(reference, link, available) {
  if (link.startsWith("kicad-embed://")) {
    return "";
  }
  const kicad = /^\$\{KICAD[0-9]+_3DMODEL_DIR\}[/\\](.*)$/.exec(link);
  if (kicad !== null && !kicad[1].split(/[/\\]/).includes("..")) {
    return "";
  }
  const colon = reference.indexOf(":");
  const library = reference.substring(0, colon);
  const name = reference.substring(colon + 1);
  const own = "${GITPLM_PARTS}/3d-models/" + library + ".3dshapes/" + name + ".step";
  const ours = link.startsWith("${GITPLM_PARTS}/") || link.startsWith("${GITPLM_PARTS}\\");
  const candidate = ours ? classifyModelReference(link).candidate : null;
  if (candidate === null) {
    return `the footprint's 3D model link ${link} is outside the Catalog's library, so it only works on one computer. Link the model as ${own} in KiCad's Footprint Editor, or remove the link, then choose the file again`;
  }
  if (!isCatalogModelKey(candidate)) {
    return `the footprint links the 3D model ${candidate}, a place where the Catalog keeps no 3D models. Link the model as ${own} in KiCad's Footprint Editor, then choose the file again`;
  }
  if (!available(candidate)) {
    return `the footprint links the 3D model ${candidate}, which the Catalog does not have. Choose that 3D model file too, or fix the link in KiCad's Footprint Editor`;
  }
  return "";
}

// Where the quoted string starting at `start` (its ") ends, just past its
// closing ", with backslash escapes; the end of the text if not closed.
function quotedStringEnd(text, start) {
  let index = start + 1;
  while (index < text.length) {
    const character = text.charAt(index);
    if (character === "\\") {
      index += 2;
    } else if (character === '"') {
      return index + 1;
    } else {
      index += 1;
    }
  }
  return text.length;
}

// Every 3D model path of a footprint's text, as written: each quoted
// (model "...") path and each KiCad 5 path written without quotes, outside
// quoted strings (crates/catalog download::model_links). Only the model
// link rule reads these; snapshots keep modelReferences, which leaves the
// unquoted ones out, so their hashes never change.
function modelLinks(text) {
  const links = [];
  const space = (character) => /\s/.test(character);
  let index = 0;
  while (index < text.length) {
    const character = text.charAt(index);
    if (character === '"') {
      index = quotedStringEnd(text, index);
      continue;
    }
    if (character !== "(") {
      index += 1;
      continue;
    }
    let head = index + 1;
    while (head < text.length && space(text.charAt(head))) {
      head += 1;
    }
    const after = head + "model".length;
    index += 1;
    const next = text.charAt(after);
    if (text.substring(head, after) !== "model" || !(space(next) || next === '"')) {
      continue;
    }
    let start = after;
    while (start < text.length && space(text.charAt(start))) {
      start += 1;
    }
    let end;
    if (text.charAt(start) === '"') {
      end = quotedStringEnd(text, start);
      links.push(text.substring(start + 1, Math.max(start + 1, end - 1)));
    } else {
      end = start;
      while (end < text.length && !space(text.charAt(end)) && text.charAt(end) !== "(" && text.charAt(end) !== ")") {
        end += 1;
      }
      if (end > start) {
        links.push(text.substring(start, end));
      }
    }
    index = Math.max(end, index);
  }
  return links;
}

// The name after the colon of LIB:NAME, or null.
function referenceName(reference) {
  const colon = reference.indexOf(":");
  return colon > 0 && colon < reference.length - 1 ? reference.substring(colon + 1) : null;
}

// Reads an uploaded symbol: one top-level (symbol "NAME" ...) list, alone,
// whose NAME is the reference's name. Throws a plain BadRequestError.
function readUploadedSymbol(upload) {
  const text = bytesText(upload.bytes);
  const tokens = sexpTokens(text);
  const name = referenceName(upload.reference);
  const one = tokens !== null && sexpChildren(tokens) !== null;
  if (!one || text.charAt(0) !== "(" || text.charAt(text.length - 1) !== ")" || sexpWord(tokens[1]) !== "symbol") {
    fail(
      `the file for the symbol ${upload.reference} must hold exactly one symbol, (symbol "NAME" ...), and nothing else`,
    );
  }
  if (name === null || sexpWord(tokens[2]) !== bytesText(toBytes(name))) {
    fail(`the symbol in the file is not named ${name === null ? upload.reference : name}, the name in ${upload.reference}`);
  }
  if (sexpChildren(tokens).some((child) => sexpWord(child[1]) === "extends")) {
    fail(`the symbol ${upload.reference} extends another symbol; choose a symbol that does not`);
  }
}

// Reads an uploaded footprint: a whole .kicad_mod whose (footprint "NAME"
// ...) (or older (module ...)) is the reference's name. Gives its format
// version and its 3D model references, each classified.
function readUploadedFootprint(upload) {
  const text = bytesText(upload.bytes);
  const tokens = sexpTokens(text);
  const children = tokens === null ? null : sexpChildren(tokens);
  const head = tokens === null ? null : sexpWord(tokens[1]);
  if (children === null || (head !== "footprint" && head !== "module")) {
    fail(`the file for the footprint ${upload.reference} is not a KiCad footprint (.kicad_mod)`);
  }
  const name = referenceName(upload.reference);
  if (name === null || sexpWord(tokens[2]) !== bytesText(toBytes(name))) {
    fail(
      `the footprint in the file is not named ${name === null ? upload.reference : name}, the name in ${upload.reference}`,
    );
  }
  const version = children.find((child) => sexpWord(child[1]) === "version");
  return {
    formatVersion: version ? sexpWord(version[2]) : null,
    models: modelReferences(upload.text).map((reference) => {
      const classified = classifyModelReference(reference);
      // Only a path into a footprint library's own 3D model folder can name a
      // Catalog 3D model; anything else stays a missing local model.
      const candidate =
        classified.candidate !== null && isCatalogModelKey(classified.candidate) ? classified.candidate : null;
      return { reference: reference, scope: classified.scope, status: classified.status, candidate: candidate };
    }),
  };
}

// A new CAD item revision's snapshot document, with keys in the order
// crates/catalog CadItemDetails writes them.
function cadItemSnapshot(kind, key, formatVersion, sha256, size, modelReferences) {
  return JSON.stringify({
    schema: CAD_ITEM_DETAILS_SCHEMA,
    kind: kind,
    key: key,
    format: CAD_FORMATS[kind],
    format_version: formatVersion,
    blob_sha256: sha256,
    blob_size: size,
    model_references: modelReferences,
    license: { status: UNKNOWN_LICENSE },
  });
}

// What each upload of an edit becomes, read and checked against what is
// stored: by "kind\nkey", { kind, key, cadItemId, cadItemIsNew, isNew,
// number, id, predecessor, snapshot, snapshotSha256, content, upload } for a
// new revision, or { isNew: false, current, content } when the content is
// the CAD item's current revision's. 3D models come first, so a footprint
// names their revisions; a footprint reference into the Catalog's
// 3d-models folder binds the 3D model uploaded with it, or the Catalog's
// own, or stays unresolved ("missing") when neither has it. Every 3D model
// upload must be one that an uploaded footprint names.
function preparedUploads(txApp, uploads) {
  const prepared = {};
  const named = {};
  const footprints = {};
  for (const upload of uploads) {
    if (upload.kind === "model-3d") {
      if (!isCatalogModelKey(upload.reference)) {
        fail(
          `${upload.reference} is not a Catalog 3D model path, such as 3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step`,
        );
      }
      continue;
    }
    const problem = uploadReferenceProblem(upload.reference);
    if (problem) {
      fail(problem);
    }
    if (upload.kind === "symbol") {
      readUploadedSymbol(upload);
    } else {
      footprints[upload.reference] = readUploadedFootprint(upload);
      for (const model of footprints[upload.reference].models) {
        if (model.candidate) {
          named[model.candidate] = true;
        }
      }
    }
  }
  for (const upload of uploads) {
    if (upload.kind === "model-3d" && !named[upload.reference]) {
      fail(`no footprint uploaded with the 3D model ${upload.reference} names it`);
    }
  }
  const cadItem = (kind, key) => {
    const id = deriveId("cad-item", kind + "\n" + key);
    const stored = recordById(txApp, "catalog_cad_items", id);
    if (stored && (stored.getString("kind") !== kind || stored.getString("key") !== key)) {
      throw new BadRequestError(`the stored CAD item ${id} is not the ${kind} ${key}`);
    }
    return { id: id, stored: stored };
  };
  const prepare = (upload, formatVersion, modelTargets) => {
    const item = cadItem(upload.kind, upload.reference);
    const current = item.stored ? currentCadItemRevision(txApp, item.id) : null;
    const snapshot = cadItemSnapshot(
      upload.kind,
      upload.reference,
      formatVersion,
      upload.sha256,
      upload.bytes.length,
      modelTargets,
    );
    const content = cadContent(snapshot);
    if (current && cadContent(current.getString("snapshot")) === content) {
      return { isNew: false, current: current, cadItemId: item.id, content: content };
    }
    const number = current ? current.getInt("number") + 1 : 1;
    return {
      isNew: true,
      kind: upload.kind,
      key: upload.reference,
      cadItemId: item.id,
      cadItemIsNew: !item.stored,
      number: number,
      id: deriveId("cad-item-revision", item.id + "\n" + number),
      predecessor: current ? current.id : "",
      snapshot: snapshot,
      snapshotSha256: $security.sha256(snapshot),
      content: content,
      upload: upload,
    };
  };
  // The revision a binding to `entry` names.
  const bindingOf = (entry) =>
    entry.isNew
      ? { kind: "cad-item-revision", cad_item: entry.cadItemId, number: entry.number, snapshot_sha256: entry.snapshotSha256 }
      : bindingTo(entry.cadItemId, entry.current);
  for (const upload of uploads) {
    if (upload.kind === "model-3d") {
      prepared[upload.kind + "\n" + upload.reference] = prepare(upload, null, []);
    }
  }
  for (const upload of uploads) {
    if (upload.kind === "symbol") {
      prepared[upload.kind + "\n" + upload.reference] = prepare(upload, upload.formatVersion, []);
    } else if (upload.kind === "footprint") {
      const read = footprints[upload.reference];
      // KiCad 5 paths written without quotes count too.
      for (const link of modelLinks(upload.text)) {
        const problem = modelLinkProblem(upload.reference, link, (key) => {
          if (prepared["model-3d\n" + key]) {
            return true;
          }
          const stored = cadItem("model-3d", key);
          return stored.stored !== null && stored.stored !== undefined && currentCadItemRevision(txApp, stored.id) !== null;
        });
        if (problem) {
          fail(problem);
        }
      }
      // The 3D model revisions the footprint's current revision binds,
      // kept while their content is the 3D model's current content.
      const item = cadItem("footprint", upload.reference);
      const currentFootprint = item.stored ? currentCadItemRevision(txApp, item.id) : null;
      const kept = {};
      if (currentFootprint) {
        for (const model of JSON.parse(currentFootprint.getString("snapshot")).model_references || []) {
          if (model && model.target && model.target.kind === "cad-item-revision") {
            kept[model.target.cad_item] = model.target;
          }
        }
      }
      const targets = read.models.map((model) => {
        let target;
        const sent = model.candidate ? prepared["model-3d\n" + model.candidate] : undefined;
        if (sent) {
          target = bindingOf(sent);
        } else if (model.candidate) {
          const stored = cadItem("model-3d", model.candidate);
          const current = stored.stored ? currentCadItemRevision(txApp, stored.id) : null;
          if (current) {
            const before = kept[stored.id];
            const keptRecord = before
              ? recordById(txApp, "catalog_cad_item_revisions", deriveId("cad-item-revision", stored.id + "\n" + before.number))
              : null;
            target =
              keptRecord &&
              keptRecord.getString("snapshot_sha256") === before.snapshot_sha256 &&
              cadContent(keptRecord.getString("snapshot")) === cadContent(current.getString("snapshot"))
                ? bindingTo(stored.id, keptRecord)
                : bindingTo(stored.id, current);
          } else {
            target = { kind: "unresolved", status: "missing" };
          }
        } else {
          target = { kind: "unresolved", status: model.status };
        }
        return { reference: model.reference, scope: model.scope, target: target };
      });
      prepared[upload.kind + "\n" + upload.reference] = prepare(upload, read.formatVersion, targets);
    }
  }
  return prepared;
}

// The upload's new CAD item revision that `target` names, or undefined.
function newUploadFor(prepared, target) {
  for (const key of Object.keys(prepared)) {
    const entry = prepared[key];
    if (entry.isNew && entry.cadItemId === target.cad_item && entry.number === target.number) {
      return entry;
    }
  }
  return undefined;
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

// Why `details` (parsed part details) cannot be saved, or "": its category
// must be a category code, its fields and manufacturer parts must be pairs
// of text, and no field may be Symbol or Footprint.
function detailsProblem(details) {
  if (typeof details.category !== "string" || !CATEGORY_CODE.test(details.category)) {
    return "its category must be the code of a category, such as res";
  }
  const fields = details.fields;
  if (!Array.isArray(fields) || !fields.every((field) => isPair(field, ["name", "value"]))) {
    return "its fields must each be a name and a value, both text";
  }
  const cad = fields.find((field) => CAD_FIELDS.indexOf(field.name) >= 0);
  if (cad) {
    return `it lists ${cad.name}, which belongs to the part's revision, not its details`;
  }
  const named = fields.find((field) => isCategoryName(field.name));
  if (named) {
    return `it lists a field named ${named.name}, but Category is the part's category in this app`;
  }
  const parts = details.manufacturer_parts;
  if (!Array.isArray(parts) || !parts.every((part) => isPair(part, ["manufacturer", "mpn"]))) {
    return "its manufacturer parts must each be a manufacturer and an MPN, both text";
  }
  return "";
}

// ---- Categories ---------------------------------------------------------------
//
// A category is made by the CSV import, one per CSV file (g-res makes res),
// and saved as numbered revisions like a CAD item; its current revision is
// its newest. A part's category is the code in its part details.

// A category code: the CSV file name without g-, such as res.
const CATEGORY_CODE = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;

// The category a CSV file makes, as crates/catalog category_code does.
function categoryCode(csvFile) {
  return csvFile.indexOf("g-") === 0 ? csvFile.substring(2) : csvFile;
}

// The label of a category difference, and the name a CSV review accepts to
// take the CSV's category (crates/catalog CATEGORY_LABEL).
const CATEGORY_FIELD = "Category";

// Whether a field name is Category in any case, with spaces around it
// (crates/catalog part_details_columns refuses such a column). It is the
// part's category in this app, never a field, so a CSV review's Category
// always means the category.
function isCategoryName(name) {
  return typeof name === "string" && trimSpace(name).toLowerCase() === CATEGORY_FIELD.toLowerCase();
}

// A list of at most `max` distinct, non-empty texts of at most `length`
// characters, or null.
function textList(value, max, length) {
  if (!Array.isArray(value) || value.length > max) {
    return null;
  }
  const seen = {};
  for (const item of value) {
    if (typeof item !== "string" || trimSpace(item).length === 0 || item.length > length || seen["#" + item]) {
      return null;
    }
    seen["#" + item] = true;
  }
  return value.slice();
}

// A category revision's snapshot rebuilt in the key order of crates/catalog
// CategoryDetails, so its JSON is the canonical text; or a string saying
// why `parsed` is not a category.
function categoryDocument(parsed) {
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return "it is not a category";
  }
  const keys = Object.keys(parsed).sort().join(",");
  if (keys !== "code,fields,ipn_prefix,name,rules,schema" || parsed.schema !== CATEGORY_SCHEMA) {
    return `it must be ${CATEGORY_SCHEMA} with code, name, ipn_prefix, fields and rules`;
  }
  if (typeof parsed.code !== "string" || !CATEGORY_CODE.test(parsed.code)) {
    return "its code must be a plain name, such as res";
  }
  if (typeof parsed.name !== "string" || trimSpace(parsed.name).length === 0 || parsed.name.length > 200) {
    return "its name must be text of 1 to 200 characters";
  }
  if (typeof parsed.ipn_prefix !== "string" || !/^([A-Z]{3})?$/.test(parsed.ipn_prefix)) {
    return "its IPN prefix must be three capital letters, or blank";
  }
  const fields = textList(parsed.fields, 200, 500);
  if (!fields) {
    return "its fields must be a list of different names";
  }
  if (fields.some((name) => CAD_FIELDS.indexOf(name) >= 0)) {
    return "its fields cannot include Symbol or Footprint, which belong to a part's revision";
  }
  if (fields.some((name) => isCategoryName(name))) {
    return "its fields cannot include Category, which is the part's category in this app";
  }
  const rules = parsed.rules;
  if (
    !rules ||
    typeof rules !== "object" ||
    Array.isArray(rules) ||
    Object.keys(rules).sort().join(",") !== "footprint_patterns,keywords,reference_letters"
  ) {
    return "its rules must be reference letters, footprint patterns and keywords";
  }
  const letters = textList(rules.reference_letters, 100, 50);
  const patterns = textList(rules.footprint_patterns, 200, 200);
  const keywords = textList(rules.keywords, 200, 200);
  if (!letters || !patterns || !keywords) {
    return "each of its rules must be a list of different texts";
  }
  return {
    schema: CATEGORY_SCHEMA,
    code: parsed.code,
    name: parsed.name,
    ipn_prefix: parsed.ipn_prefix,
    fields: fields,
    rules: { reference_letters: letters, footprint_patterns: patterns, keywords: keywords },
  };
}

// The newest (current) revision of the category with `code`, or null.
function currentCategoryRevision(txApp, code) {
  const found = txApp.findRecordsByFilter("catalog_category_revisions", "category = {:category}", "-number", 1, 0, {
    category: deriveId("category", code),
  });
  return found.length > 0 ? found[0] : null;
}

// A footprint's family, as crates/catalog footprint_family works it out:
// its name without the library, up to the first digit, without trailing
// _ - . or space. g-Resistor_SMD:R_0603_1608Metric gives R.
function footprintFamily(footprint) {
  const at = footprint.lastIndexOf(":");
  const name = trimSpace(at >= 0 ? footprint.substring(at + 1) : footprint);
  const digit = name.search(/[0-9]/);
  return (digit >= 0 ? name.substring(0, digit) : name).replace(/[_\-. ]+$/, "");
}

// Reference letters that mean the same as another (crates/catalog
// REFERENCE_ALIASES).
const REFERENCE_ALIASES = Object.freeze(
  Object.assign(Object.create(null), { IC: "U", S: "SW", X: "Y", XTAL: "Y", VR: "RV", POT: "RV", CN: "J" }),
);

// The letters of a symbol reference in capitals, aliases applied, as
// crates/catalog reference_letter: U5 gives U, s1 gives SW.
function referenceLetter(reference) {
  const letters = /^[A-Za-z#]*/.exec(trimSpace(reference))[0].toUpperCase();
  return REFERENCE_ALIASES[letters] || letters;
}

// The value of a symbol's own (property "Reference" "...") in the bytes
// of a symbol CAD item revision (text from bytesText), or "".
function symbolReference(text) {
  const children = sexpChildren(sexpTokens("(kicad_symbol_lib " + text + ")"));
  const entry = children ? children.find((child) => sexpWord(child[1]) === "symbol") : null;
  const inner = entry ? sexpChildren(entry) : null;
  const property = inner
    ? inner.find((item) => sexpWord(item[1]) === "property" && sexpWord(item[2]) === "Reference")
    : null;
  const value = property ? sexpWord(property[3]) : null;
  return typeof value === "string" ? value : "";
}

// The clues a person confirms with a part's category: its footprint family
// (from the first footprint choice), its reference letter (from its symbol,
// when the Catalog has the symbol's bytes) and its main manufacturer. Each
// is { clue, value }, blanks left out. `bytesOf(target)` gives the bytes of
// a bound symbol CAD item revision, or null.
function categoryClues(details, revision, bytesOf) {
  const clues = [];
  const add = (clue, value) => {
    const text = trimSpace(value || "").substring(0, 500);
    if (text.length > 0) {
      clues.push({ clue: clue, value: text });
    }
  };
  const references = revision.cad_references || [];
  const footprint = references.find((item) => item && item.role === "footprint");
  add("footprint-family", footprint ? footprintFamily(footprint.reference) : "");
  const symbol = references.find((item) => item && item.role === "symbol");
  const target = symbol && symbol.target && symbol.target.kind === "cad-item-revision" ? symbol.target : null;
  const bytes = target ? bytesOf(target) : null;
  add("reference-letter", bytes ? referenceLetter(symbolReference(bytesText(bytes))) : "");
  const main = (details.manufacturer_parts || [])[0];
  add("manufacturer", main ? main.manufacturer : "");
  return clues;
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

// ---- Numbered records ----------------------------------------------------------
//
// A part's revisions and details, a CAD item's revisions and a category's
// revisions are numbered from 1 for each owner, and none is ever deleted.
// So a new one must come right after the newest: numbered one higher,
// naming the newest as its predecessor, under the ID derived from its owner
// and number. The write guards check this for every new one.

// For each numbered collection: the field naming the owner, the ID kind,
// and the words its messages use.
const NUMBERED_RECORDS = Object.freeze({
  catalog_part_revisions: {
    ownerField: "part",
    ownerWord: "part",
    idKind: "part-revision",
    newOne: "a new revision",
    oneWord: "revision",
    newestWords: "its newest revision",
  },
  catalog_part_details: {
    ownerField: "part",
    ownerWord: "part",
    idKind: "part-details",
    newOne: "new details",
    oneWord: "details",
    newestWords: "its current details",
  },
  catalog_cad_item_revisions: {
    ownerField: "cad_item",
    ownerWord: "CAD item",
    idKind: "cad-item-revision",
    newOne: "a new revision",
    oneWord: "revision",
    newestWords: "its current revision",
  },
  catalog_category_revisions: {
    ownerField: "category",
    ownerWord: "category",
    idKind: "category-revision",
    newOne: "a new revision",
    oneWord: "revision",
    newestWords: "its current revision",
  },
});

// Why the new numbered `record` does not come right after its owner's
// newest one, or "" if it does.
function numberedRecordProblem(txApp, record) {
  const rule = NUMBERED_RECORDS[record.collection().name];
  const owner = record.getString(rule.ownerField);
  const number = record.getInt("number");
  const found = txApp.findRecordsByFilter(
    record.collection().name,
    rule.ownerField + " = {:owner}",
    "-number",
    1,
    0,
    { owner: owner },
  );
  const newest = found.length > 0 ? found[0] : null;
  const expected = newest ? newest.getInt("number") + 1 : 1;
  if (number !== expected || record.getString("predecessor") !== (newest ? newest.id : "")) {
    return `${rule.newOne} of ${rule.ownerWord} ${owner} must be number ${expected}, right after ${rule.newestWords}`;
  }
  if (record.id !== deriveId(rule.idKind, owner + "\n" + number)) {
    return `${rule.oneWord} ${number} of ${rule.ownerWord} ${owner} must have the ID derived from its ${rule.ownerWord} and number`;
  }
  return "";
}

// ---- Every new record names its own transaction's Change -----------------------
//
// Desktops follow the Catalog Change by Change (crates/catalog/src/replica.rs).
// For each new Change, a desktop fetches the records that name it, once. A
// record that names an older Change would never reach a desktop that had
// already applied that Change. So the write guards hold every save to two
// rules:
//
// - A transaction saves at most one Change, numbered right after the
//   newest.
// - Every new record that names a Change names the one saved in the same
//   transaction. A part's current details or revision may move only to
//   what that Change saved.
//
// How a guard finds the Change saved in this transaction: a transaction
// sees its own writes at once, but other database connections see them
// only once it is committed. In a hook, e.app is the transaction. The app
// PocketBase started with reads through its own connections, so it sees
// committed records only. The newest Change the transaction sees is its
// own exactly when that app cannot find it. Outside a transaction both
// see the same records, so no Change counts as this save's own.
//
// "The Change with the highest seq" is not enough on its own: server code
// that saved no Change would pass by naming the newest committed one.
//
// A refused save is always a programming error in server code, never a
// person's mistake, so it answers 500 and is logged as an error.

const COMMITTED_APP = "catalog.committedApp";

// Remembers the app PocketBase started with, so guards can read committed
// records. catalog_write_guards.pb.js calls it once, at start-up.
function rememberCommittedApp(app) {
  app.store().set(COMMITTED_APP, app);
}

// The newest Change `app` can see, or null when there is none.
function newestChange(app) {
  const found = app.findRecordsByFilter("catalog_changes", "seq > 0", "-seq", 1, 0);
  return found.length > 0 ? found[0] : null;
}

// The Change saved earlier in the transaction `txApp` runs, or null when
// it saved none or `txApp` is not a transaction.
function changeOfThisTransaction(txApp) {
  const committed = txApp.store().get(COMMITTED_APP);
  if (!committed) {
    refuseProgrammingError(
      txApp,
      "the write guards cannot read committed records, because the app was not remembered at start-up.",
    );
  }
  const newest = newestChange(txApp);
  if (!newest || recordById(committed, "catalog_changes", newest.id)) {
    return null;
  }
  return newest;
}

// The fields of `collection` that name a Change: each relation field to
// the catalog_changes collection, as { name, required }. Fields are read
// from the schema, so a new collection or field is covered without being
// listed anywhere.
function changeFieldsOf(collection, changesCollectionId) {
  const found = [];
  const fields = collection.fields;
  for (let i = 0; i < fields.length; i++) {
    const field = fields[i];
    if (field.type() === "relation" && field.collectionId === changesCollectionId) {
      found.push({ name: field.name, required: field.required });
    }
  }
  return found;
}

// Why a new record breaks the rule that it names its own transaction's
// Change, or "" if it keeps it. `fields` lists its Change fields as
// { name, required, value }. `ownChangeId` is the ID of the Change this
// transaction saved, or "" when it saved none. An optional field may stay
// empty, as an audit entry's does for an import run that saved no Change.
function changeFieldsProblem(collectionName, fields, ownChangeId) {
  for (const field of fields) {
    if (field.value === "" && !field.required) {
      continue;
    }
    if (ownChangeId !== "" && field.value === ownChangeId) {
      continue;
    }
    const named = field.value === "" ? "no Change" : `Change ${field.value}`;
    if (ownChangeId === "") {
      return `a new ${collectionName} record names ${named} in ${field.name}, but this transaction saved no Change. Save a new Change first, in the same transaction.`;
    }
    return `a new ${collectionName} record names ${named} in ${field.name}. It must name Change ${ownChangeId}, which this transaction saved.`;
  }
  return "";
}

// Why a new Change numbered `seq` breaks the numbering, or "" if it does
// not. `newest` is the newest Change the transaction sees, as
// { id, seq, own }, or null; `own` says this transaction saved it.
function newChangeProblem(seq, newest) {
  if (newest && newest.own) {
    return `this transaction already saved Change ${newest.id}. A transaction saves one Change, so desktops fetch all its records together.`;
  }
  const expected = newest ? newest.seq + 1 : 1;
  if (seq !== expected) {
    return `a new Change must be numbered ${expected}, right after the newest, not ${seq}. Desktops refuse gaps in the numbering.`;
  }
  return "";
}

// Refuses a save that a write guard caught. Only wrong server code gets
// here, so the message says so. The request answers 500 with the message.
// It is also logged as an error twice: in the dashboard's Logs, and in the
// server's own output, which is where a service manager keeps it.
function refuseProgrammingError(app, problem) {
  const message = `Programming error in the team server, not a user mistake: ${problem} The save was refused. See catalog_write_guards.pb.js.`;
  app.logger().error(message);
  console.error(message);
  throw new InternalServerError(message);
}

// Refuses a new record that names a Change other than its own
// transaction's (see the rules above).
function requireOwnChange(txApp, record) {
  const collection = record.collection();
  let changes = null;
  try {
    changes = txApp.findCollectionByNameOrId("catalog_changes");
  } catch (_) {
    // Before the Catalog migrations run, no field can name a Change.
    return;
  }
  const fields = changeFieldsOf(collection, changes.id);
  if (fields.length === 0) {
    return;
  }
  const own = changeOfThisTransaction(txApp);
  const problem = changeFieldsProblem(
    collection.name,
    fields.map((field) => ({ name: field.name, required: field.required, value: record.getString(field.name) })),
    own ? own.id : "",
  );
  if (problem) {
    refuseProgrammingError(txApp, problem);
  }
}

// Refuses a new Change that is not the first in its transaction or that
// is not numbered right after the newest.
function requireNextChange(txApp, change) {
  const newest = newestChange(txApp);
  // This transaction's Change, when there is one, is always the newest.
  const own = changeOfThisTransaction(txApp) !== null;
  const problem = newChangeProblem(
    change.getInt("seq"),
    newest ? { id: newest.id, seq: newest.getInt("seq"), own: own } : null,
  );
  if (problem) {
    refuseProgrammingError(txApp, problem);
  }
}

// Refuses a move of `part`'s current details or revision that the Change
// saved in this transaction did not make. `moved` says which pointers
// moved; the part-update guard has already checked where they point.
// Details move only to details that Change saved. A revision moves only to
// a revision that Change saved, or because of an archive entry in it.
function requireMoveByOwnChange(txApp, part, moved) {
  const own = changeOfThisTransaction(txApp);
  const ipn = part.getString("ipn");
  if (moved.details) {
    const details = recordById(txApp, "catalog_part_details", part.getString("current_details"));
    if (!own || !details || details.getString("change") !== own.id) {
      refuseProgrammingError(
        txApp,
        `the current details of ${ipn} moved to details that this transaction's Change did not save. Desktops would never hear of the move.`,
      );
    }
  }
  if (moved.revision) {
    const revision = recordById(txApp, "catalog_part_revisions", part.getString("current_revision"));
    const savedByOwn = own !== null && revision !== null && revision.getString("change") === own.id;
    const archivedByOwn =
      own !== null &&
      txApp.findRecordsByFilter("catalog_archive_entries", "part = {:part} && change = {:change}", "", 1, 0, {
        part: part.id,
        change: own.id,
      }).length > 0;
    if (!savedByOwn && !archivedByOwn) {
      refuseProgrammingError(
        txApp,
        `the current revision of ${ipn} moved, but this transaction's Change saved neither that revision nor an archive entry. Desktops would never hear of the move.`,
      );
    }
  }
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
// - a reference with a file uploaded for it (`prepared`, from
//   preparedUploads) binds the new CAD item revision the file makes, or,
//   when the file has the content of the CAD item's current revision,
//   keeps the revision it binds now if that has the same content and
//   otherwise binds the current one, as the import does;
// - anything else is refused: an edit can only use CAD items the Catalog
//   already has, or files sent with it.
// Other libraries (such as KiCad's own) are external, as in the import.
function cadReferences(txApp, fields, currentRevision, prepared) {
  prepared = prepared || {};
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
    // KiCad gives a part one symbol, and a footprint choice listed twice
    // is a slip; an edit may not make either. CSV-sourced CAD is kept and
    // flagged with an import problem instead (several-symbols,
    // repeated-footprint-choice), so a field the edit leaves as stored is
    // not checked: an old CSV slip never blocks an unrelated CAD edit.
    const stored = (currentRevision.cad_fields || []).find((field) => field.name === column);
    const changed = !stored || stored.value !== text;
    if (changed && role === "symbol" && names.length > 1) {
      throw new BadRequestError(
        `a part has one symbol, so the Symbol field can name only one; it names ${names.length}`,
      );
    }
    const repeated = names.find((name, index) => names.indexOf(name) !== index);
    if (changed && repeated) {
      throw new BadRequestError(`the ${column} field lists ${repeated} twice`);
    }
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
        const uploaded = prepared[role + "\n" + reference];
        if (uploaded) {
          uploaded.used = true;
        }
        const keptRecord =
          same && same.target.kind === "cad-item-revision" && same.target.cad_item === cadItemId
            ? recordById(txApp, "catalog_cad_item_revisions", deriveId("cad-item-revision", cadItemId + "\n" + same.target.number))
            : null;
        if (uploaded && uploaded.isNew) {
          target = {
            kind: "cad-item-revision",
            cad_item: uploaded.cadItemId,
            number: uploaded.number,
            snapshot_sha256: uploaded.snapshotSha256,
          };
        } else if (uploaded) {
          // The file has the content of the CAD item's current revision.
          target =
            keptRecord &&
            keptRecord.getString("snapshot_sha256") === same.target.snapshot_sha256 &&
            cadContent(keptRecord.getString("snapshot")) === uploaded.content
              ? bindingTo(cadItemId, keptRecord)
              : bindingTo(cadItemId, uploaded.current);
        } else if (cadItem && same && same.target.kind === "cad-item-revision" && same.target.cad_item === cadItemId) {
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
            `the ${role} ${reference} is not in the Catalog; choose a file for it, or use a ${role} the Catalog already has`,
          );
        }
      }
      references.push({ role: role, ordinal: ordinal, reference: reference, target: target });
    });
  }
  for (const key of Object.keys(prepared)) {
    const entry = prepared[key];
    const [kind, reference] = [key.substring(0, key.indexOf("\n")), key.substring(key.indexOf("\n") + 1)];
    if (kind !== "model-3d" && !entry.used) {
      throw new BadRequestError(
        `the file sent for the ${kind} ${reference} is not used: the ${kind === "symbol" ? "Symbol" : "Footprint"} field does not name ${reference}`,
      );
    }
  }
  return references;
}

// ---- Excluded from BOM ---------------------------------------------------------
//
// Excluded from BOM comes only from KiCad's own flags in the bound CAD's
// stored bytes (kicad-parts-ccc0, mx-9a51aa): a part is excluded when a
// symbol it binds says (in_bom no), or when every footprint choice is a
// footprint whose (attr ...) has exclude_from_bom. A reference that is not
// bound to a stored CAD item revision (KiCad's own library, or a missing
// symbol or footprint) gives no evidence. The server works it out itself for
// every new part revision, by import, edit or CSV review; it never takes the
// value from a client. crates/kicad-integration library_file.rs and the
// zero-loss report read the flags the same way.

// The tokens of KiCad s-expression text: "(" and ")", { text } for a
// quoted string, and { atom } for anything else. The text holds one
// character per byte (bytesText), and only ASCII space, tab, LF, FF and CR
// separate tokens, exactly as crates/kicad-integration library_file.rs reads
// bytes; null if a string is not closed.
function sexpTokens(text) {
  const tokens = [];
  let index = 0;
  while (index < text.length) {
    const char = text[index];
    if (char === "(" || char === ")") {
      tokens.push(char);
      index++;
    } else if (char === '"') {
      let value = "";
      index++;
      while (index < text.length && text[index] !== '"') {
        if (text[index] === "\\") {
          value += text[index + 1] || "";
          index += 2;
        } else {
          value += text[index];
          index++;
        }
      }
      if (index >= text.length) {
        return null;
      }
      index++;
      tokens.push({ text: value });
    } else if (/[ \t\n\f\r]/.test(char)) {
      index++;
    } else {
      const start = index;
      while (index < text.length && !/[ \t\n\f\r()"]/.test(text[index])) {
        index++;
      }
      tokens.push({ atom: text.substring(start, index) });
    }
  }
  return tokens;
}

// The word a token stands for (an atom or a string), or null.
function sexpWord(token) {
  if (token && typeof token === "object") {
    return token.atom !== undefined ? token.atom : token.text;
  }
  return null;
}

// The direct child lists of the root list of `tokens`, each as the tokens
// it holds (its own parentheses included), or null if the text is not one
// balanced list.
function sexpChildren(tokens) {
  if (!tokens || tokens[0] !== "(") {
    return null;
  }
  const children = [];
  let depth = 0;
  let start = -1;
  for (let position = 0; position < tokens.length; position++) {
    const token = tokens[position];
    if (token === "(") {
      depth++;
      if (depth === 2) {
        start = position;
      }
    } else if (token === ")") {
      if (depth === 0) {
        return null;
      }
      if (depth === 2) {
        children.push(tokens.slice(start, position + 1));
      }
      depth--;
      if (depth === 0 && position + 1 !== tokens.length) {
        return null;
      }
    }
  }
  return depth === 0 ? children : null;
}

// Whether a symbol CAD item revision's bytes (one or more top-level
// (symbol ...) entries, as text from bytesText) have an entry that says
// (in_bom no). Like SymbolLibrary::parse, any entry without a name makes
// the whole text unreadable, so false.
function symbolSaysNotInBom(text) {
  const children = sexpChildren(sexpTokens("(kicad_symbol_lib " + text + ")"));
  if (!children) {
    return false;
  }
  const entries = children.filter((child) => sexpWord(child[1]) === "symbol");
  if (entries.some((entry) => sexpWord(entry[2]) === null)) {
    return false;
  }
  return entries.some((entry) => {
    const inner = sexpChildren(entry);
    if (!inner) {
      return false;
    }
    const inBom = inner.find((item) => sexpWord(item[1]) === "in_bom");
    return !!inBom && sexpWord(inBom[2]) === "no";
  });
}

// Whether a footprint CAD item revision's bytes (a whole .kicad_mod, as text
// from bytesText) have exclude_from_bom in an (attr ...).
function footprintExcludedFromBom(text) {
  const tokens = sexpTokens(text);
  const head = tokens && sexpWord(tokens[1]);
  const children = sexpChildren(tokens);
  if (!children || (head !== "footprint" && head !== "module")) {
    return false;
  }
  return children.some(
    (child) =>
      sexpWord(child[1]) === "attr" &&
      child.some((token) => token && typeof token === "object" && token.atom === "exclude_from_bom"),
  );
}

// Bytes (a JS array of numbers or a Go byte slice) as text with one
// character per byte, so a byte that is not UTF-8 stays one character, as
// the Rust reader sees it.
function bytesText(bytes) {
  const parts = [];
  const length = bytes.length;
  for (let start = 0; start < length; start += 8192) {
    const chunk = [];
    const end = Math.min(start + 8192, length);
    for (let i = start; i < end; i++) {
      chunk.push(bytes[i] & 0xff);
    }
    parts.push(String.fromCharCode.apply(null, chunk));
  }
  return parts.join("");
}

// Whether `bytes` of a symbol or footprint say it is not in the BOM.
function bomMark(role, bytes) {
  const text = bytesText(bytes);
  return role === "symbol" ? symbolSaysNotInBom(text) : footprintExcludedFromBom(text);
}

// Excluded from BOM for a part revision document. `markOf(target, role)`
// says whether a bound symbol or footprint CAD item revision says it is not
// in the BOM: true, false, or null when it cannot be read.
function excludedFromBom(document, markOf) {
  let symbolMarked = false;
  const footprints = [];
  for (const reference of document.cad_references || []) {
    const target = reference && reference.target;
    const role = reference && reference.role;
    if (role !== "symbol" && role !== "footprint") {
      continue;
    }
    const mark = target && target.kind === "cad-item-revision" ? markOf(target, role) === true : false;
    if (role === "symbol") {
      symbolMarked = symbolMarked || mark;
    } else {
      footprints.push(mark);
    }
  }
  const marked = footprints.filter((choice) => choice).length;
  return symbolMarked || (marked > 0 && marked === footprints.length);
}

// The stored bytes of a blob record.
function blobBytes(record) {
  const fsys = $app.newFilesystem();
  try {
    const reader = fsys.getReader(record.baseFilesPath() + "/" + record.getString("file"));
    try {
      return toBytes(reader);
    } finally {
      reader.close();
    }
  } finally {
    fsys.close();
  }
}

// markOf for excludedFromBom over stored CAD item revisions: only a symbol
// or footprint CAD item revision of the bound role is read, and each blob is
// read and judged at most once.
function storedBomMarks(txApp) {
  const marks = {};
  return (target, role) => {
    const revision = recordById(
      txApp,
      "catalog_cad_item_revisions",
      deriveId("cad-item-revision", target.cad_item + "\n" + target.number),
    );
    if (!revision || !isCadItemRevision(revision, target.cad_item, target.number)) {
      return null;
    }
    let kind = null;
    try {
      kind = JSON.parse(revision.getString("snapshot")).kind;
    } catch (_) {
      kind = null;
    }
    if (kind !== role) {
      return null;
    }
    const key = role + "\n" + revision.getString("blob");
    if (marks[key] === undefined) {
      const blob = recordById(txApp, "catalog_blobs", revision.getString("blob"));
      marks[key] = blob ? bomMark(role, blobBytes(blob)) : null;
    }
    return marks[key];
  };
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

// Saves the new CAD item revisions an edit's or a new part's files make
// (entries from newEntries), 3D models first, with their blobs and new CAD
// items. Blobs and CAD items are shared by content and identity, so one that
// is already stored is reused once it is shown to be the same. Each
// revision's source names the file it came from.
function saveNewCadItemRevisions(txApp, entries, changeId, source) {
  const record = (name, id) => {
    const made = new Record(txApp.findCollectionByNameOrId(name));
    made.set("id", id);
    return made;
  };
  for (const entry of entries) {
    const blobId = deriveId("blob", entry.upload.sha256);
    const storedBlob = recordById(txApp, "catalog_blobs", blobId);
    if (storedBlob && storedBlob.getString("sha256") !== entry.upload.sha256) {
      throw new Error(`the stored blob ${blobId} is not the bytes with SHA-256 ${entry.upload.sha256}`);
    }
    if (!storedBlob) {
      const blob = record("catalog_blobs", blobId);
      blob.set("sha256", entry.upload.sha256);
      blob.set("size", entry.upload.bytes.length);
      blob.set("file", $filesystem.fileFromBytes(entry.upload.bytes, entry.upload.sha256 + ".bin"));
      blob.set("first_change", changeId);
      txApp.save(blob);
    }
    if (entry.cadItemIsNew) {
      const cadItem = record("catalog_cad_items", entry.cadItemId);
      cadItem.set("kind", entry.kind);
      cadItem.set("key", entry.key);
      cadItem.set("first_change", changeId);
      txApp.save(cadItem);
    }
    const made = record("catalog_cad_item_revisions", entry.id);
    made.set("cad_item", entry.cadItemId);
    made.set("number", entry.number);
    made.set("predecessor", entry.predecessor);
    made.set("change", changeId);
    made.set("blob", blobId);
    made.set("snapshot_sha256", entry.snapshotSha256);
    made.set("snapshot", entry.snapshot);
    made.set("license", UNKNOWN_LICENSE);
    const library = entry.upload.library;
    const installed = entry.upload.installed;
    made.set(
      "source",
      JSON.stringify(
        library
          ? {
              schema: SOURCE_SCHEMA,
              source: LIBRARY_SOURCE,
              path: library.path,
              byte_sha256: library.byteSha256,
              source_commit: library.commit,
              byte_range: library.byteRange,
              file_name: entry.upload.fileName,
            }
          : installed
            ? {
                schema: SOURCE_SCHEMA,
                source: INSTALLED_SOURCE,
                library: installed.library,
                path: installed.path,
                file_name: entry.upload.fileName,
              }
            : { schema: SOURCE_SCHEMA, source: source, file_name: entry.upload.fileName },
      ),
    );
    txApp.save(made);
  }
}

// The new CAD item revisions of `prepared` (from preparedUploads), 3D models
// first, as their footprints name them.
function newEntries(prepared) {
  return Object.keys(prepared)
    .map((key) => prepared[key])
    .filter((entry) => entry.isNew)
    .sort((a, b) => (a.kind === "model-3d" ? 0 : 1) - (b.kind === "model-3d" ? 0 : 1));
}

// Saves one remembered match per clue a part has (categoryClues) for the
// category a person chose, in the Change that saved their choice, and gives
// the clues saved. `what` holds category, details and revision (the parsed
// documents the part has after the Change), prepared (the Change's uploads),
// partId, changeId, seq and actor. The match create guard checks each.
function saveRememberedMatches(txApp, what) {
  const bytesOf = (target) => {
    const sent = newUploadFor(what.prepared, target);
    if (sent) {
      return sent.kind === "symbol" ? sent.upload.bytes : null;
    }
    const stored = recordById(
      txApp,
      "catalog_cad_item_revisions",
      deriveId("cad-item-revision", target.cad_item + "\n" + target.number),
    );
    if (!stored || !isCadItemRevision(stored, target.cad_item, target.number)) {
      return null;
    }
    const blob = recordById(txApp, "catalog_blobs", stored.getString("blob"));
    return blob ? blobBytes(blob) : null;
  };
  const remembered = [];
  for (const clue of categoryClues(what.details, what.revision, bytesOf)) {
    const made = new Record(txApp.findCollectionByNameOrId("catalog_category_matches"));
    made.set("id", deriveId("category-match", what.changeId + "\n" + clue.clue));
    made.set("category", deriveId("category", what.category));
    made.set("clue", clue.clue);
    made.set("value", clue.value);
    made.set("part", what.partId);
    made.set("change", what.changeId);
    made.set("seq", what.seq);
    made.set("actor", what.actor.id);
    made.set("actor_name", what.actor.name);
    txApp.save(made);
    remembered.push(clue);
  }
  return remembered;
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
            request.details.category,
            pairs(request.details.fields),
            request.details.manufacturerParts.map((part) => [part && part.manufacturer, part && part.mpn]),
          ]
        : null,
      request.cad
        ? [
            request.cad.expected,
            pairs(request.cad.fields),
            request.cad.uploads.map(hashedUpload),
          ]
        : null,
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
    // Set when the edit changes the part's category: the clues a person
    // confirmed with it are remembered.
    let newCategory = null;
    if (request.details) {
      const storedFields = storedDetails.fields || [];
      let category = storedDetails.category;
      let expectedFields = storedFields;
      if (request.details.category !== null && request.details.category !== storedDetails.category) {
        // A new category keeps every field the part has and adds, blank, the
        // fields of the new category the part lacks, in the category's
        // order; the IPN never changes.
        const categoryRevision = currentCategoryRevision(txApp, request.details.category);
        if (!categoryRevision) {
          throw new BadRequestError(`the category ${request.details.category} is not in the Catalog`);
        }
        const categoryDetails = JSON.parse(categoryRevision.getString("snapshot"));
        const have = storedFields.map((field) => field.name);
        expectedFields = storedFields.concat(
          (categoryDetails.fields || [])
            .filter((name) => have.indexOf(name) < 0)
            .map((name) => ({ name: name, value: "" })),
        );
        category = request.details.category;
        newCategory = category;
      }
      const document = {
        schema: PART_DETAILS_SCHEMA,
        ipn: request.ipn,
        category: category,
        fields: editedFields(request.details.fields, expectedFields),
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
    // The CAD item revisions the uploaded files make, by "kind\nkey".
    let prepared = {};
    if (request.cad) {
      const fields = editedFields(request.cad.fields, storedRevision.cad_fields || []);
      prepared = preparedUploads(txApp, request.cad.uploads);
      const document = {
        schema: PART_REVISION_SCHEMA,
        ipn: request.ipn,
        cad_fields: fields,
        cad_references: cadReferences(txApp, fields, storedRevision, prepared),
      };
      // Excluded from BOM follows the CAD the new revision binds, read by
      // the server from the stored bytes (kicad-parts-ccc0), or from the
      // bytes uploaded for a new CAD item revision. Like PartRevision, the
      // snapshot names it only when it is set.
      const stored = storedBomMarks(txApp);
      const markOf = (target, role) => {
        const sent = newUploadFor(prepared, target);
        if (!sent) {
          return stored(target, role);
        }
        return sent.kind === role ? bomMark(role, sent.upload.bytes) : null;
      };
      if (excludedFromBom(document, markOf)) {
        document.excluded_from_bom = true;
      }
      refuse("revision", revisionProblem(document));
      const snapshot = JSON.stringify(document);
      const sha = $security.sha256(snapshot);
      // Only CAD that really changes can bring an archived design back; an
      // unchanged CAD section (sent with a details edit) never does.
      if (sha !== currentRevision.getString("snapshot_sha256")) {
        const storedContent = storedCadContent(txApp);
        const archived = archivedRevisionWithContent(
          txApp,
          partId,
          partRevisionContent(document, (target) => {
            const sent = newUploadFor(prepared, target);
            return sent ? sent.content : storedContent(target);
          }),
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
      // The new CAD item revisions the uploaded files make, 3D models
      // first, as their footprints name them. A new CAD item revision is
      // only ever saved with the part revision that binds it.
      const newCadItemRevisions = revision ? newEntries(prepared) : [];
      const summary = JSON.stringify({
        ipn: request.ipn,
        partDetails: details ? 1 : 0,
        partRevisions: revision ? 1 : 0,
        cadItemRevisions: newCadItemRevisions.length,
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

      saveNewCadItemRevisions(txApp, newCadItemRevisions, changeId, UPLOAD_SOURCE);

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

      // The category the person chose is remembered with the part's clues,
      // from the details and revision it has after this edit.
      const remembered =
        newCategory !== null && details
          ? saveRememberedMatches(txApp, {
              category: newCategory,
              details: JSON.parse(details.snapshot),
              revision: revision ? JSON.parse(revision.snapshot) : storedRevision,
              prepared: prepared,
              partId: partId,
              changeId: changeId,
              seq: seq,
              actor: actor,
            })
          : [];

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
        // The clues remembered with a new category, each { clue, value }.
        rememberedMatches: remembered,
        cadItemRevisions: bound,
        // Each new CAD item revision, with how many other parts still use
        // an older revision of that symbol or footprint (they keep their
        // exact binding; kicad-parts-10a7 offers to move them).
        newCadItemRevisions: newCadItemRevisions.map((entry) => ({
          kind: entry.kind,
          key: entry.key,
          number: entry.number,
          id: entry.id,
          otherParts:
            entry.kind === "model-3d"
              ? null
              : txApp.findRecordsByFilter(
                  "catalog_part_revisions",
                  "part.current_revision = id && part != {:part} && cad_item_revisions.cad_item ?= {:item}",
                  "",
                  0,
                  0,
                  { part: partId, item: entry.cadItemId },
                ).length,
        })),
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

// ---- New parts --------------------------------------------------------------
//
// POST /api/catalog/parts (see catalog_new_parts.pb.js) adds one new part
// from CAD files someone downloaded (kicad-parts-9693). It runs
// publishCreate in one transaction:
//
//   1. replay the stored answer if the correlation ID was used before for
//      the same request (409 correlation_reused for a different one);
//   2. answer 409 ipn_taken if the Catalog already has a part with the IPN,
//      or if a row of the latest import run of a CSV file has it
//      (csvRowKeptWithIpn);
//   3. build part details 1 in the category, with exactly its fields, and
//      revision 1 binding exact CAD item revisions: ones the Catalog
//      already has, KiCad's own libraries (external), or ones made from the
//      files sent, read and checked as for an edit;
//   4. save one Change of kind create, the CAD item revisions, the part,
//      its details and revision, an audit entry, and one remembered match
//      per clue the part has for the category the person confirmed.
//
// Applied answers are stored under the correlation ID; ipn_taken is not, so
// a retry with another IPN is checked again.

// The `source` of a new part's details, revision and CAD item revisions.
const CREATE_SOURCE = "create";

// The row of the latest import run of a CSV file that has `ipn` but did not
// become a part, as { path }, or null (kicad-parts-3b72).
//
// Every other row of a latest run is a part already, or names a part that
// is (duplicate-ipn, ipn-given-to-another-part), so the "already has a
// part" check covers those. Two kinds of row keep an IPN without a part:
// - a row with more fields than headers (row-has-extra-fields);
// - a row whose IPN is valid but for spaces around it (invalid-ipn), such
//   as "RES-0003-0001 " from a spreadsheet, because the import reads IPNs
//   exactly (crates/catalog ipn.rs).
// Someone meant that IPN for a part, so giving it to another part in the
// app would make two parts with one IPN once the row is fixed. Only the
// latest run of each CSV file counts: an older run's row may be gone.
//
// IPNs the CSV catalog has but no import has read yet are not here: only
// the desktop app can see those, in its own folder (see
// MOVE_THE_CSV_CATALOG.md on giving new IPNs in one place).
function csvRowKeptWithIpn(txApp, ipn) {
  const kept = txApp.findRecordsByFilter(
    "catalog_import_problems",
    "(code = 'row-has-extra-fields' || code = 'invalid-ipn') && detail ~ {:ipn}",
    "",
    0,
    0,
    { ipn: ipn },
  );
  const latestRuns = {};
  for (const problem of kept) {
    let detail;
    try {
      detail = JSON.parse(problem.getString("detail"));
    } catch (_) {
      continue;
    }
    const column = Array.isArray(detail.headers) ? detail.headers.indexOf("IPN") : -1;
    const value = column >= 0 && Array.isArray(detail.record) ? detail.record[column] : undefined;
    if (typeof value !== "string" || trimSpace(value) !== ipn) {
      continue;
    }
    const csvFile = csvFileOf(problem.getString("path"));
    if (latestRuns[csvFile] === undefined) {
      const found = txApp.findRecordsByFilter(
        "catalog_audit_entries",
        "csv_file = {:csv_file} && csv_import_number > 0",
        "-csv_import_number",
        1,
        0,
        { csv_file: csvFile },
      );
      latestRuns[csvFile] = found.length > 0 ? JSON.parse(found[0].getString("sources")) : null;
    }
    const run = latestRuns[csvFile];
    if (run && Array.isArray(run.problems) && run.problems.indexOf(problem.id) >= 0) {
      return { path: problem.getString("path") };
    }
  }
  return null;
}

// The checked request of a new part.
function createRequest(body) {
  const { correlationId, reason, ipn } = commonRequest(body);
  const category = body.category;
  if (typeof category !== "string" || !CATEGORY_CODE.test(category)) {
    fail("category must be the code of a category, such as res");
  }
  const list = (name, max) => {
    const value = body[name] === undefined || body[name] === null ? [] : body[name];
    if (!Array.isArray(value) || value.length > max) {
      fail(`${name} must be a list of at most ${max}`);
    }
    return value;
  };
  const fields = list("fields", 200);
  const manufacturerParts = list("manufacturerParts", 100);
  const cadFields = list("cadFields", 2);
  const from = body.from === undefined || body.from === null ? "" : body.from;
  if (typeof from !== "string" || from.length > 200) {
    fail("from must be text of at most 200 characters");
  }
  const files = list("files", 20).map((name) => {
    if (typeof name !== "string" || trimSpace(name).length === 0 || name.length > 255) {
      fail("files must be the names of the downloaded files, each of at most 255 characters");
    }
    return name;
  });
  return {
    correlationId: correlationId,
    reason: reason,
    ipn: ipn,
    category: category,
    fields: fields,
    manufacturerParts: manufacturerParts,
    cadFields: cadFields,
    uploads: editUploads(body.uploads),
    from: trimSpace(from),
    files: files,
  };
}

// The fields of a new part: exactly `names` (its category's fields), in that
// order, each with a text value.
function createdFields(fields, names) {
  if (fields.length !== names.length) {
    fail(`fields must list the category's ${names.length} fields, in its order: ${names.join(", ")}`);
  }
  return fields.map((field, index) => {
    if (!field || field.name !== names[index]) {
      fail(`field ${index + 1} must be ${names[index]}, the category's field ${index + 1}`);
    }
    if (typeof field.value !== "string" || field.value.length > 10000) {
      fail(`the value of ${field.name} must be text of at most 10000 characters`);
    }
    return { name: field.name, value: field.value };
  });
}

// The Symbol and Footprint fields of a new part, in that order. Every
// reference they name must be LIB:NAME with plain names, as for a file.
function createdCadFields(fields) {
  const checked = fields.map((field, index) => {
    if (!field || CAD_FIELDS.indexOf(field.name) < 0 || typeof field.value !== "string" || field.value.length > 10000) {
      fail(`cad field ${index + 1} must be Symbol or Footprint, with a text value`);
    }
    return { name: field.name, value: field.value };
  });
  for (const field of checked) {
    for (const reference of namedReferences(field.value)) {
      const parts = reference.split(":");
      if (parts.length !== 2 || !isPlainName(parts[0]) || !isPlainName(parts[1])) {
        fail(
          `${reference} cannot be used as a reference: write LIBRARY:NAME with one colon, and a library and name that are not . or .., do not start with -, and hold no / \\ : or control characters`,
        );
      }
    }
  }
  return checked;
}

function publishCreate(e) {
  const kind = CHANGE_KINDS.create;
  const actor = catalogEditor(e);
  const request = createRequest(e.requestInfo().body || {});
  const pairs = (fields) => fields.map((field) => [field && field.name, field && field.value]);
  const requestHash = $security.sha256(
    JSON.stringify([
      kind,
      request.ipn,
      request.reason,
      request.category,
      pairs(request.fields),
      request.manufacturerParts.map((part) => [part && part.manufacturer, part && part.mpn]),
      pairs(request.cadFields),
      request.uploads.map(hashedUpload),
      request.from,
      request.files,
    ]),
  );

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
    const existing =
      recordById(txApp, "catalog_parts", partId) || first("catalog_parts", "ipn = {:ipn}", { ipn: request.ipn });
    if (existing) {
      // Who added it: when it is the caller (a save whose answer was lost,
      // then changed and sent again), the app offers to open it instead of
      // saving the part twice under another IPN.
      const added = recordById(txApp, "catalog_changes", existing.getString("first_change"));
      reply = {
        status: 409,
        body: {
          outcome: "ipn_taken",
          correlationId: request.correlationId,
          ipn: request.ipn,
          message: `the Catalog already has a part with the IPN ${request.ipn}; an IPN is never reused`,
          addedByYou: !!added && added.getString("kind") === CHANGE_KINDS.create && added.getString("actor") === actor.id,
        },
      };
      return;
    }
    const keptRow = csvRowKeptWithIpn(txApp, request.ipn);
    if (keptRow) {
      reply = {
        status: 409,
        body: {
          outcome: "ipn_taken",
          correlationId: request.correlationId,
          ipn: request.ipn,
          message: `A row of ${keptRow.path} has the IPN ${request.ipn}, but the import could not make a part from it. An IPN is never reused.`,
          addedByYou: false,
          csvFile: keptRow.path,
        },
      };
      return;
    }
    const categoryRevision = currentCategoryRevision(txApp, request.category);
    if (!categoryRevision) {
      throw new BadRequestError(`the category ${request.category} is not in the Catalog`);
    }
    const categoryDetails = JSON.parse(categoryRevision.getString("snapshot"));
    // A category with an IPN prefix numbers its parts with it.
    const prefix = categoryDetails.ipn_prefix || "";
    if (prefix && request.ipn.substring(0, 3) !== prefix) {
      fail(`a part in ${categoryDetails.name} has an IPN starting with ${prefix}-, not ${request.ipn}`);
    }
    const refuse = (what, problem) => {
      if (problem) {
        throw new BadRequestError(`the ${what} of ${request.ipn} cannot be saved: ${problem}`);
      }
    };

    // The documents, in the field order of crates/catalog PartDetails and
    // PartRevision, so the stored bytes are their canonical JSON.
    const detailsDocument = {
      schema: PART_DETAILS_SCHEMA,
      ipn: request.ipn,
      category: request.category,
      fields: createdFields(request.fields, categoryDetails.fields || []),
      manufacturer_parts: editedManufacturerParts(request.manufacturerParts),
    };
    refuse("details", detailsProblem(detailsDocument));
    const cadFields = createdCadFields(request.cadFields);
    const prepared = preparedUploads(txApp, request.uploads);
    // A new part makes new CAD items. A file named like a CAD item the
    // Catalog has, with other content, would change what every part using
    // it gets, so it is refused; the same content simply uses it.
    for (const key of Object.keys(prepared)) {
      const entry = prepared[key];
      if (entry.isNew && !entry.cadItemIsNew) {
        const what = entry.kind === "model-3d" ? "3D model" : entry.kind;
        fail(
          `the Catalog already has the ${what} ${entry.key}, with other content; a new part cannot change it. Use the Catalog's ${what}, or put the file's ${what} in another library`,
        );
      }
    }
    const revisionDocument = {
      schema: PART_REVISION_SCHEMA,
      ipn: request.ipn,
      cad_fields: cadFields,
      cad_references: cadReferences(txApp, cadFields, { cad_fields: [], cad_references: [] }, prepared),
    };
    const stored = storedBomMarks(txApp);
    const markOf = (target, role) => {
      const sent = newUploadFor(prepared, target);
      if (!sent) {
        return stored(target, role);
      }
      return sent.kind === role ? bomMark(role, sent.upload.bytes) : null;
    };
    if (excludedFromBom(revisionDocument, markOf)) {
      revisionDocument.excluded_from_bom = true;
    }
    refuse("revision", revisionProblem(revisionDocument));
    const detailsSnapshot = JSON.stringify(detailsDocument);
    const revisionSnapshot = JSON.stringify(revisionDocument);

    const record = (name, id) => {
      const made = new Record(txApp.findCollectionByNameOrId(name));
      made.set("id", id);
      return made;
    };
    const last = first("catalog_changes", "seq > 0", {}, "-seq");
    const seq = last ? last.getInt("seq") + 1 : 1;
    const changeId = deriveId("change", request.correlationId);
    const newCadItemRevisions = newEntries(prepared);
    const summary = JSON.stringify({
      ipn: request.ipn,
      parts: 1,
      partDetails: 1,
      partRevisions: 1,
      cadItemRevisions: newCadItemRevisions.length,
    });
    const change = record("catalog_changes", changeId);
    change.set("seq", seq);
    change.set("kind", kind);
    change.set("reason", request.reason);
    change.set("correlation_id", request.correlationId);
    change.set("actor", actor.id);
    change.set("actor_name", actor.name);
    change.set("sources", JSON.stringify({ parts: [request.ipn], from: request.from, files: request.files }));
    change.set("summary", summary);
    txApp.save(change);

    saveNewCadItemRevisions(txApp, newCadItemRevisions, changeId, CREATE_SOURCE);

    // Where the part came from: the site and the downloaded files.
    const source = JSON.stringify({
      schema: SOURCE_SCHEMA,
      source: CREATE_SOURCE,
      from: request.from,
      files: request.files,
    });
    const part = record("catalog_parts", partId);
    part.set("ipn", request.ipn);
    part.set("ipn_prefix", request.ipn.substring(0, 3));
    part.set("variant", request.ipn.substring(9));
    part.set("current_revision_number", 1);
    part.set("current_details_number", 1);
    part.set("first_change", changeId);
    txApp.save(part);
    const detailsId = deriveId("part-details", partId + "\n1");
    const details = record("catalog_part_details", detailsId);
    details.set("part", partId);
    details.set("number", 1);
    details.set("predecessor", "");
    details.set("change", changeId);
    details.set("snapshot", detailsSnapshot);
    details.set("snapshot_sha256", $security.sha256(detailsSnapshot));
    details.set("source", source);
    txApp.save(details);
    const revisionId = deriveId("part-revision", partId + "\n1");
    const bound = boundCadItemRevisions(revisionDocument);
    const revision = record("catalog_part_revisions", revisionId);
    revision.set("part", partId);
    revision.set("number", 1);
    revision.set("predecessor", "");
    revision.set("change", changeId);
    revision.set("snapshot", revisionSnapshot);
    revision.set("snapshot_sha256", $security.sha256(revisionSnapshot));
    revision.set("source", source);
    revision.set("cad_item_revisions", bound);
    txApp.save(revision);
    part.set("current_details", detailsId);
    part.set("current_revision", revisionId);
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

    // The person confirmed the category: its clues are remembered.
    const remembered = saveRememberedMatches(txApp, {
      category: request.category,
      details: detailsDocument,
      revision: revisionDocument,
      prepared: prepared,
      partId: partId,
      changeId: changeId,
      seq: seq,
      actor: actor,
    });

    const result = {
      outcome: "applied",
      correlationId: request.correlationId,
      ipn: request.ipn,
      changeId: changeId,
      seq: seq,
      auditEntryId: auditEntryId,
      details: { id: detailsId, number: 1, snapshotSha256: $security.sha256(detailsSnapshot) },
      revision: { id: revisionId, number: 1, snapshotSha256: $security.sha256(revisionSnapshot) },
      rememberedMatches: remembered,
      cadItemRevisions: bound,
      newCadItemRevisions: newCadItemRevisions.map((entry) => ({
        kind: entry.kind,
        key: entry.key,
        number: entry.number,
        id: entry.id,
        otherParts:
          entry.kind === "model-3d"
            ? null
            : txApp.findRecordsByFilter(
                "catalog_part_revisions",
                "part.current_revision = id && part != {:part} && cad_item_revisions.cad_item ?= {:item}",
                "",
                0,
                0,
                { part: partId, item: entry.cadItemId },
              ).length,
      })),
    };
    const ledger = new Record(txApp.findCollectionByNameOrId("catalog_commands"));
    ledger.set("correlation_id", request.correlationId);
    ledger.set("actor", actor.id);
    ledger.set("request_hash", requestHash);
    ledger.set("status", 200);
    ledger.set("result", JSON.stringify(result));
    txApp.save(ledger);
    reply = { status: 200, body: result };
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

// ---- A CSV row for another part (kicad-parts-3b72) ----------------------------
//
// While a team moves from the CSV catalog to the app, both can give out new
// IPNs. So one IPN can reach two different parts: a part someone added in
// the app, and a CSV row someone else wrote without seeing it. An import
// must never take such a row as new values for the app's part. If it did, a
// CSV review could turn that part into a different one.
//
// The rule: a CSV row may be another part when the part with its IPN was
// added in the app (its first Change is a create), the row names none of
// that part's MPNs, and the row is not an exact copy of the part. Then:
// - one shared MPN is enough to count as one part, even if the other MPNs
//   differ. A shared MPN is trusted; a generic MPN shared by two packages
//   (LM358 in SOIC and in DIP) still counts as one part, and the CSV review
//   of the differences is the guard (ADR 0008);
// - a row whose part details and CAD are exactly the part's is that part,
//   whatever its MPN: it is a copy, such as a test point with no MPN copied
//   into g-art.csv;
// - when the row or the part has no MPN, nothing else shows they are one
//   part, so the row is flagged. A wrong guess overwrites a part; a wrong
//   flag only asks a person.
// The row may still be the same part, with a corrected or differently
// spelled MPN (LT1763CS8#PBF against LT1763CS8#TRPBF). So the message never
// says they differ: it says nothing shows they are one, and gives both ways
// out.
// A part an import made is never flagged: its IPN came from the CSV
// catalog, which is where the row is.
//
// MPNs compare without regard to case or the spaces around them, as Add
// part's MPN check does (crates/catalog-browser same_mpn). A placeholder
// such as "-", "N/A" or "TBD" is no MPN.

// The import problem code of such a row. The row is kept whole in the
// problem, and the part is left as it is.
const IPN_GIVEN_TO_ANOTHER_PART = "ipn-given-to-another-part";

// MPN cells that say there is no MPN yet, compared in capitals.
const PLACEHOLDER_MPNS = ["-", "--", "–", "—", "?", "N/A", "NA", "TBD", "TBC", "NONE", "NULL", "UNKNOWN"];

// The MPNs of a list of manufacturer parts, as written, without blanks and
// placeholders.
function mpnsOf(manufacturerParts) {
  return (Array.isArray(manufacturerParts) ? manufacturerParts : [])
    .map((part) => (part && typeof part.mpn === "string" ? trimSpace(part.mpn) : ""))
    .filter((mpn) => mpn !== "" && PLACEHOLDER_MPNS.indexOf(mpn.toUpperCase()) < 0);
}

// Whether a CSV row may be a different part from the stored part with its
// IPN (see above). `part` is { firstChangeKind, manufacturerParts } of the
// stored part, `row` is { manufacturerParts, sameDetailsAndCad }: whether
// the row's part details and CAD are exactly the part's current ones.
function csvRowIsAnotherPart(part, row) {
  if (part.firstChangeKind !== CHANGE_KINDS.create || row.sameDetailsAndCad) {
    return false;
  }
  const partMpns = mpnsOf(part.manufacturerParts).map((mpn) => mpn.toUpperCase());
  return !mpnsOf(row.manufacturerParts).some((mpn) => partMpns.indexOf(mpn.toUpperCase()) >= 0);
}

// The plain message of an IPN_GIVEN_TO_ANOTHER_PART problem: which row, by
// its IPN and MPN (row numbers differ by one between files and Excel),
// which part has the IPN in the team catalog, why nothing shows they are
// one part, and what to do either way. It holds nothing that changes from
// one import to the next, because problems are stored by content.
function anotherPartMessage(row, ipn, part) {
  const rowMpns = mpnsOf(row.manufacturerParts);
  const partMpns = mpnsOf(part.manufacturerParts);
  const list = (mpns) => mpns.join(", ");
  const theRowHas = rowMpns.length === 0 ? "has no MPN" : rowMpns.length === 1 ? `has MPN ${rowMpns[0]}` : `has MPNs ${list(rowMpns)}`;
  const named = part.description ? `: ${part.description}` : "";
  const why =
    partMpns.length === 0
      ? "That part has no MPN, and the row differs from it, so nothing shows they are one part."
      : `That part's ${partMpns.length === 1 ? "MPN is" : "MPNs are"} ${list(partMpns)}, so nothing shows they are one part.`;
  const ifSame =
    partMpns.length === 0
      ? "If it is that part, make the row match it exactly, or remove the row."
      : `If it is that part, put that part's MPN (${partMpns[0]}) in the row, or remove the row.`;
  return [
    `In ${row.path}, the row with IPN ${ipn} ${theRowHas}.`,
    `The team catalog already gave ${ipn} to a part added in the app${named}.`,
    why,
    "The import left that part as it is and kept the row here.",
    "If the row is a different part, give it another IPN.",
    ifSame,
  ].join(" ");
}

// The IPN_GIVEN_TO_ANOTHER_PART problem of a CSV import's row, as
// { message, detail }, or null when the row may be the part (see above).
// `stored` is { firstChangeKind, details } of the part with the row's IPN
// (its current part details, parsed); `row` is { ipn, path, raw_record,
// substitutesCsvRows, manufacturerParts, sameDetailsAndCad }.
function anotherPartProblem(stored, row) {
  const flagged = csvRowIsAnotherPart(
    { firstChangeKind: stored.firstChangeKind, manufacturerParts: stored.details.manufacturer_parts },
    row,
  );
  if (!flagged) {
    return null;
  }
  const description = (stored.details.fields || []).find((field) => field.name === "Description");
  return {
    message: anotherPartMessage(row, row.ipn, {
      description: description ? description.value : "",
      manufacturerParts: stored.details.manufacturer_parts,
    }),
    detail: JSON.stringify({
      path: row.path,
      row_number: row.rowNumber,
      raw_record: row.rawRecord,
      substitutes_csv_rows: row.substitutesCsvRows,
      csv_manufacturer_parts: row.manufacturerParts,
      part_manufacturer_parts: stored.details.manufacturer_parts,
    }),
  };
}

// ---- Reviewing CSV changes that were not applied ------------------------------
//
// POST /api/catalog/csv-reviews (see catalog_csv_reviews.pb.js): a person
// reviews one csv-details-differ-from-edited-details or
// csv-cad-differs-from-edited-revision import problem, taking the CSV's
// values for the fields they name and keeping theirs for the rest. What is
// saved is built from the problem's recorded CSV document, never from the
// client: the client sends only field names, the number it expects, a
// reason and a correlation ID. Taking details saves the part's next details
// (no revision); taking Symbol or Footprint adds a revision; taking nothing
// records that the person kept their values. Each review is one Change of
// kind csv-review with one catalog_csv_reviews record.
//
// A csv-cad-differs-from-edited-cad-item problem (kicad-parts-3b8b) says the
// CSV's file for a symbol, footprint or 3D model the part uses changed,
// while that CAD item's current revision was made in the app. Its CSV file
// is not stored, so it can only be kept; taking it is refused with a plain
// message.

// The import problem whose CSV file for a CAD item made in the app changed.
const CSV_CAD_ITEM_KEPT = "csv-cad-differs-from-edited-cad-item";

// The kind of each import problem a person can review.
const CSV_REVIEW_KINDS = Object.freeze({
  "csv-details-differ-from-edited-details": "details",
  "csv-cad-differs-from-edited-revision": "cad",
  [CSV_CAD_ITEM_KEPT]: "cad",
});

// The CSV document a reviewable import problem recorded (its parsed
// detail): the CSV's part details, its part revision, or, for
// CSV_CAD_ITEM_KEPT, its snapshot of the CAD item. A review covers its
// SHA-256.
function recordedCsvDocument(code, detail) {
  if (code === CSV_CAD_ITEM_KEPT) {
    return detail.csv_cad_item;
  }
  return CSV_REVIEW_KINDS[code] === "details" ? detail.details : detail.revision;
}

// The field name a person picks to take the CSV's manufacturer parts.
const MANUFACTURER_PARTS = "Manufacturer parts";

// The CSV file an import problem's path names, such as g-res for
// database/g-res.csv.
function csvFileOf(path) {
  const match = /^database\/([^/]+)\.csv$/.exec(path);
  return match ? match[1] : "";
}

function publishCsvReview(e) {
  const kind = CHANGE_KINDS.csvReview;
  const actor = catalogEditor(e);
  const body = e.requestInfo().body || {};
  const request = commonRequest(body);
  const problemId = body.problem;
  if (typeof problemId !== "string" || !/^[0-9a-z]{15}$/.test(problemId)) {
    fail("problem must be the ID of an import problem");
  }
  const accept = body.accept;
  if (
    !Array.isArray(accept) ||
    accept.length > 200 ||
    !accept.every((name) => typeof name === "string" && name.length > 0 && name.length <= 500)
  ) {
    fail("accept must be a list of field names");
  }
  if (new Set(accept).size !== accept.length) {
    fail("accept names a field twice");
  }
  const expected = body.expected;
  if (typeof expected !== "number" || !Number.isInteger(expected) || expected < 1) {
    fail("expected must be the number of the part's current details or revision");
  }
  const requestHash = $security.sha256(
    JSON.stringify([kind, request.ipn, request.reason, problemId, accept, expected]),
  );

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
    const problem = recordById(txApp, "catalog_import_problems", problemId);
    const reviewKind = problem ? CSV_REVIEW_KINDS[problem.getString("code")] : undefined;
    if (!problem || !reviewKind || problem.getString("subject") !== request.ipn) {
      throw new NotFoundError(`${request.ipn} has no such CSV change to review`);
    }
    const detail = JSON.parse(problem.getString("detail"));
    const csvSnapshot = recordedCsvDocument(problem.getString("code"), detail);
    if (typeof csvSnapshot !== "string") {
      throw new Error(`import problem ${problemId} records no CSV document`);
    }
    const csvSha = $security.sha256(csvSnapshot);
    const currentDetails = recordById(txApp, "catalog_part_details", part.getString("current_details"));
    const currentRevision = recordById(txApp, "catalog_part_revisions", part.getString("current_revision"));

    // The latest import run of the problem's CSV file must have found it,
    // or the CSV has moved on since.
    const csvFile = csvFileOf(problem.getString("path"));
    const latest = first(
      "catalog_audit_entries",
      "csv_file = {:csv_file} && csv_import_number > 0",
      { csv_file: csvFile },
      "-csv_import_number",
    );
    const run = latest ? JSON.parse(latest.getString("sources")) : null;
    if (!run || !Array.isArray(run.problems) || run.problems.indexOf(problemId) < 0) {
      reply = {
        status: 409,
        body: {
          outcome: "out_of_date",
          correlationId: request.correlationId,
          ipn: request.ipn,
          message: `the CSV catalog changed after this difference was found; import ${csvFile}.csv again to see what differs now`,
        },
      };
      return;
    }
    // The part must still be what the person read. (It may have been
    // edited since the import found the problem: the CSV's values are
    // still compared with what the part has now.)
    const current = reviewKind === "details" ? currentDetails : currentRevision;
    if (expected !== current.getInt("number")) {
      const conflict = {
        outcome: "conflict",
        correlationId: request.correlationId,
        ipn: request.ipn,
        message: `someone else changed ${request.ipn} after you read it; read it again`,
      };
      if (reviewKind === "details") {
        conflict.currentDetails = describeRecord(txApp, currentDetails);
      } else {
        conflict.currentRevision = describeRecord(txApp, currentRevision);
      }
      reply = { status: 409, body: conflict };
      return;
    }
    let status;
    let result;
    if (first("catalog_csv_reviews", "part = {:part} && code = {:code} && csv_sha256 = {:sha}", {
      part: partId,
      code: problem.getString("code"),
      sha: csvSha,
    })) {
      status = 422;
      result = {
        outcome: "noop",
        correlationId: request.correlationId,
        ipn: request.ipn,
        message: "these CSV changes were already reviewed",
      };
    } else {
      // Build the new document from the stored one and the CSV's recorded
      // values for each accepted field.
      const csv = JSON.parse(csvSnapshot);
      let details = null;
      let revision = null;
      if (reviewKind === "details" && accept.length > 0) {
        const stored = JSON.parse(currentDetails.getString("snapshot"));
        const fields = stored.fields.map((field) => ({ name: field.name, value: field.value }));
        let manufacturerParts = stored.manufacturer_parts;
        let category = stored.category;
        for (const name of accept) {
          if (name === CATEGORY_FIELD) {
            if (csv.category === stored.category) {
              fail(`the CSV's ${CATEGORY_FIELD} is the same as the part's`);
            }
            if (!currentCategoryRevision(txApp, csv.category)) {
              fail(`the category ${csv.category} is not in the Catalog`);
            }
            category = csv.category;
            continue;
          }
          if (name === MANUFACTURER_PARTS) {
            if (JSON.stringify(csv.manufacturer_parts) === JSON.stringify(stored.manufacturer_parts)) {
              fail(`the CSV's ${MANUFACTURER_PARTS} are the same as the part's`);
            }
            manufacturerParts = csv.manufacturer_parts;
            continue;
          }
          const index = fields.findIndex((field) => field.name === name);
          const fromCsv = (csv.fields || []).find((field) => field.name === name);
          if (index < 0 || !fromCsv) {
            fail(`${name} is not a field of both the part and the CSV`);
          }
          if (fields[index].value === fromCsv.value) {
            fail(`the CSV's ${name} is the same as the part's`);
          }
          fields[index] = { name: name, value: fromCsv.value };
        }
        // A new category keeps every field the part has and takes, with the
        // CSV's values, the CSV's fields the part lacks.
        if (category !== stored.category) {
          for (const field of csv.fields || []) {
            if (!fields.some((existing) => existing.name === field.name)) {
              fields.push({ name: field.name, value: field.value });
            }
          }
        }
        const document = {
          schema: PART_DETAILS_SCHEMA,
          ipn: request.ipn,
          category: category,
          fields: fields,
          manufacturer_parts: manufacturerParts,
        };
        const problemText = detailsProblem(document);
        if (problemText) {
          fail(`the new details of ${request.ipn} cannot be saved: ${problemText}`);
        }
        const snapshot = JSON.stringify(document);
        details = { snapshot: snapshot, sha: $security.sha256(snapshot) };
      }
      if (problem.getString("code") === CSV_CAD_ITEM_KEPT && accept.length > 0) {
        fail(
          `the CSV catalog's ${detail.role} file for ${detail.reference} is not in the Catalog, because the ${detail.role} was changed in this app; to use it, choose the file in Edit part`,
        );
      }
      if (reviewKind === "cad" && accept.length > 0) {
        const stored = JSON.parse(currentRevision.getString("snapshot"));
        let cadFields = stored.cad_fields.map((field) => ({ name: field.name, value: field.value }));
        let references = stored.cad_references.slice();
        for (const name of accept) {
          if (name !== "Symbol" && name !== "Footprint") {
            fail(`${name} is not the Symbol or the Footprint`);
          }
          const role = name === "Symbol" ? "symbol" : "footprint";
          const fromCsv = (csv.cad_fields || []).find((field) => field.name === name);
          const index = cadFields.findIndex((field) => field.name === name);
          if (index < 0 || !fromCsv) {
            fail(`${name} is not a field of both the part and the CSV`);
          }
          const csvReferences = (csv.cad_references || []).filter((reference) => reference.role === role);
          const storedReferences = references.filter((reference) => reference.role === role);
          if (
            cadFields[index].value === fromCsv.value &&
            canonicalJson(csvReferences) === canonicalJson(storedReferences)
          ) {
            fail(`the CSV's ${name} is the same as the part's`);
          }
          // A reference the CSV catalog cannot find never replaces the same
          // reference the Catalog has (such as a symbol added in this app,
          // which the CSV catalog's library file lacks): KiCad would lose it.
          for (const reference of csvReferences) {
            const ours = storedReferences.find((item) => item.reference === reference.reference);
            if (
              reference.target &&
              reference.target.kind === "unresolved" &&
              ours &&
              ours.target &&
              ours.target.kind === "cad-item-revision"
            ) {
              fail(
                `the CSV catalog does not have the ${role} ${reference.reference}, which the Catalog has; keep the current ${role}`,
              );
            }
          }
          // Each CAD item revision the CSV named must be stored and be what
          // the problem recorded.
          for (const reference of csvReferences) {
            const target = reference.target;
            if (target && target.kind === "cad-item-revision") {
              const record = recordById(
                txApp,
                "catalog_cad_item_revisions",
                deriveId("cad-item-revision", target.cad_item + "\n" + target.number),
              );
              if (
                !record ||
                !isCadItemRevision(record, target.cad_item, target.number) ||
                record.getString("snapshot_sha256") !== target.snapshot_sha256
              ) {
                fail(`the CSV's ${name} names a CAD item revision the Catalog does not have`);
              }
            }
          }
          cadFields[index] = { name: name, value: fromCsv.value };
          references = references.filter((reference) => reference.role !== role).concat(csvReferences);
        }
        // References in the order the fields name them: symbol, then
        // footprint.
        references.sort((a, b) => (a.role === b.role ? a.ordinal - b.ordinal : a.role === "symbol" ? -1 : 1));
        const document = {
          schema: PART_REVISION_SCHEMA,
          ipn: request.ipn,
          cad_fields: cadFields,
          cad_references: references,
        };
        // Excluded from BOM follows the CAD the new revision binds, read by
        // the server from the stored bytes (kicad-parts-ccc0).
        if (excludedFromBom(document, storedBomMarks(txApp))) {
          document.excluded_from_bom = true;
        }
        const problemText = revisionProblem(document);
        if (problemText) {
          fail(`the new revision of ${request.ipn} cannot be saved: ${problemText}`);
        }
        const archived = archivedRevisionWithContent(
          txApp,
          partId,
          partRevisionContent(document, storedCadContent(txApp)),
        );
        if (archived > 0) {
          fail(
            `this symbol and footprint are those of archived revision ${archived} of ${request.ipn}; unarchive revision ${archived} instead of saving them again`,
          );
        }
        const snapshot = JSON.stringify(document);
        revision = { snapshot: snapshot, sha: $security.sha256(snapshot) };
      }

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
          source: kind,
          problem: problemId,
          previous: { id: previous.id, number: previous.getInt("number") },
        });
      const detailsNumber = details ? currentDetails.getInt("number") + 1 : null;
      const newest = newestRevision(txApp, partId);
      const revisionNumber = revision ? newest.getInt("number") + 1 : null;
      const summary = JSON.stringify({
        ipn: request.ipn,
        problem: problemId,
        accepted: accept,
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
      if (revision) {
        const id = deriveId("part-revision", partId + "\n" + revisionNumber);
        const made = record("catalog_part_revisions", id);
        made.set("part", partId);
        made.set("number", revisionNumber);
        made.set("predecessor", newest.id);
        made.set("change", changeId);
        made.set("snapshot", revision.snapshot);
        made.set("snapshot_sha256", revision.sha);
        made.set("source", source(currentRevision));
        made.set("cad_item_revisions", boundCadItemRevisions(JSON.parse(revision.snapshot)));
        txApp.save(made);
        part.set("current_revision", id);
        part.set("current_revision_number", revisionNumber);
        savedRevision = { id: id, number: revisionNumber, snapshotSha256: revision.sha };
      }
      if (details || revision) {
        txApp.save(part);
      }

      const review = record("catalog_csv_reviews", deriveId("csv-review", request.correlationId));
      review.set("part", partId);
      review.set("problem", problemId);
      review.set("code", problem.getString("code"));
      review.set("csv_sha256", csvSha);
      review.set("accepted", JSON.stringify(accept));
      review.set("change", changeId);
      review.set("seq", seq);
      txApp.save(review);

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
        accepted: accept,
        details: savedDetails,
        revision: savedRevision,
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
  LIBRARY_SOURCE_MARK: LIBRARY_SOURCE_MARK,
  isFromOurLibraries: isFromOurLibraries,
  CATEGORY_SCHEMA: CATEGORY_SCHEMA,
  categoryCode: categoryCode,
  categoryDocument: categoryDocument,
  currentCategoryRevision: currentCategoryRevision,
  publishEdit: publishEdit,
  publishCreate: publishCreate,
  modelReferences: modelReferences,
  classifyModelReference: classifyModelReference,
  modelLinkProblem: modelLinkProblem,
  publishArchive: publishArchive,
  publishCsvReview: publishCsvReview,
  excludedFromBom: excludedFromBom,
  storedBomMarks: storedBomMarks,
  bomMark: bomMark,
  blobBytes: blobBytes,
  bytesText: bytesText,
  symbolSaysNotInBom: symbolSaysNotInBom,
  footprintExcludedFromBom: footprintExcludedFromBom,
  CSV_REVIEW_KINDS: CSV_REVIEW_KINDS,
  IPN_GIVEN_TO_ANOTHER_PART: IPN_GIVEN_TO_ANOTHER_PART,
  csvRowIsAnotherPart: csvRowIsAnotherPart,
  anotherPartMessage: anotherPartMessage,
  anotherPartProblem: anotherPartProblem,
  recordedCsvDocument: recordedCsvDocument,
  archiveState: archiveState,
  recordById: recordById,
  NUMBERED_RECORDS: NUMBERED_RECORDS,
  numberedRecordProblem: numberedRecordProblem,
  rememberCommittedApp: rememberCommittedApp,
  changeOfThisTransaction: changeOfThisTransaction,
  changeFieldsOf: changeFieldsOf,
  changeFieldsProblem: changeFieldsProblem,
  newChangeProblem: newChangeProblem,
  requireOwnChange: requireOwnChange,
  requireNextChange: requireNextChange,
  requireMoveByOwnChange: requireMoveByOwnChange,
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
  catalogEditor: catalogEditor,
  hasInvisible: hasInvisible,
  guardedCollection: guardedCollection,
  collectionChangeProblem: collectionChangeProblem,
  canonicalJson: canonicalJson,
  deriveId: deriveId,
  sha256Bytes: sha256Bytes,
  base64Decode: base64Decode,
};

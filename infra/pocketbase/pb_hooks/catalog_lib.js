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
    let formatVersion = null;
    if (role === "symbol") {
      formatVersion = upload.formatVersion;
      if (typeof formatVersion !== "string" || !/^[0-9]{1,20}$/.test(formatVersion)) {
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
      return { reference: reference, scope: classified.scope, status: classified.status, candidate: classified.candidate };
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
      continue;
    }
    if (referenceName(upload.reference) === null) {
      fail(`${upload.reference} is not a reference such as g-Resistor_SMD:R_0603_1608Metric`);
    }
    const library = upload.reference.substring(0, upload.reference.indexOf(":"));
    if (!library.startsWith("g-")) {
      fail(`a file can only be added to the Catalog's own libraries, whose names start with g-, not ${library}`);
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
      request.cad
        ? [
            request.cad.expected,
            pairs(request.cad.fields),
            request.cad.uploads.map((upload) => [
              upload.role,
              upload.reference,
              upload.fileName,
              upload.formatVersion,
              upload.sha256,
            ]),
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
      const newCadItemRevisions = revision
        ? Object.keys(prepared)
            .map((key) => prepared[key])
            .filter((entry) => entry.isNew)
            .sort((a, b) => (a.kind === "model-3d" ? 0 : 1) - (b.kind === "model-3d" ? 0 : 1))
        : [];
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

      // Blobs and CAD items are shared by content and identity, so one
      // that is already stored is reused once it is shown to be the same.
      for (const entry of newCadItemRevisions) {
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
        made.set(
          "source",
          JSON.stringify({ schema: SOURCE_SCHEMA, source: UPLOAD_SOURCE, file_name: entry.upload.fileName }),
        );
        txApp.save(made);
      }

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

// The kind of each import problem a person can review.
const CSV_REVIEW_KINDS = Object.freeze({
  "csv-details-differ-from-edited-details": "details",
  "csv-cad-differs-from-edited-revision": "cad",
});

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
    const csvSnapshot = reviewKind === "details" ? detail.details : detail.revision;
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
        for (const name of accept) {
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
        const document = {
          schema: PART_DETAILS_SCHEMA,
          ipn: request.ipn,
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
  publishEdit: publishEdit,
  modelReferences: modelReferences,
  classifyModelReference: classifyModelReference,
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

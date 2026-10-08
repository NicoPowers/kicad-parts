/// <reference path="../pb_data/types.d.ts" />

// Helpers for the Catalog routes, loaded with
// require(`${__hooks}/catalog_lib.js`) inside a handler. PocketBase does not
// load this file by itself (only *.pb.js files are hooks).

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

module.exports = { deriveId: deriveId, sha256Bytes: sha256Bytes, base64Decode: base64Decode };

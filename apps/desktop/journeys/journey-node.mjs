// Picks the Node.js that runs the journey tests, and explains the crash
// that made us pick it (kicad-parts-e24c, mulch mx-625975).
//
// On Windows, Node.js 24.15.0 sometimes kills a test process the moment
// it opens its first connection inside a test hook. The process ends with
// exit code 0xC0000409 and prints nothing. In a journey that connection is
// launchApp's request to tauri-driver, so tauri-driver logs "connection
// forcibly closed (os error 10054)" and the journey fails before any step.
// About 1 test process in 600 died this way; Node.js 24.22.0 and 22.23.3
// never did in 8,000 each. So on Windows the tests run with Node.js
// 24.22.0 or newer, downloaded once into target/journey-tools/node.

import crypto from 'node:crypto';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

/** The Node.js we download when the one running this script is too old. */
export const PINNED_NODE = {
  version: '24.22.0',
  // From https://nodejs.org/dist/v24.22.0/SHASUMS256.txt
  zipSha256: 'f9683a19877e12a9b167305c8db71040d812876359de21f0d2e97c3f321f80c3',
};

/** How Windows reports the crash: STATUS_STACK_BUFFER_OVERRUN (fast fail). */
export const NODE_CRASH_EXIT_CODE = 0xc0000409;

/**
 * Returns `{ node, why }`: the node executable the tests should run with,
 * and a sentence saying why, for the log. Downloads PINNED_NODE into
 * `toolsDir` the first time it is needed; throws if the download fails or
 * its checksum is wrong.
 */
export async function journeyNode(toolsDir) {
  const running = process.versions.node;
  if (process.platform !== 'win32') {
    return { node: process.execPath, why: `Node.js ${running} (the crash is only seen on Windows)` };
  }
  if (!olderThan(running, PINNED_NODE.version)) {
    return { node: process.execPath, why: `Node.js ${running}` };
  }
  const node = await downloadPinnedNode(toolsDir);
  return {
    node,
    why:
      `Node.js ${PINNED_NODE.version} from ${node}, ` +
      `because Node.js ${running} sometimes crashes a test process on Windows`,
  };
}

/**
 * Lists the spec files in a JUnit report whose process ended with
 * NODE_CRASH_EXIT_CODE, so the runner can say what happened in plain words.
 */
export function specsKilledByNodeCrash(junitXml) {
  const crashed = [];
  const testcase = /<testcase name="([^"]*)"[^>]*>\s*<failure[^>]*>([^<]*)/g;
  for (const [, name, failure] of junitXml.matchAll(testcase)) {
    if (failure.includes(`exitCode: ${NODE_CRASH_EXIT_CODE}`)) crashed.push(path.basename(name));
  }
  return crashed;
}

/** True when version `a` (such as "24.15.0") is older than `b`. */
function olderThan(a, b) {
  const [x, y] = [a, b].map((v) => v.split('.').map(Number));
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] < y[i];
  return false;
}

async function downloadPinnedNode(toolsDir) {
  const name = `node-v${PINNED_NODE.version}-win-x64`;
  const home = path.join(toolsDir, 'node', `v${PINNED_NODE.version}`);
  const node = path.join(home, name, 'node.exe');
  if (fs.existsSync(node)) return node;

  const url = `https://nodejs.org/dist/v${PINNED_NODE.version}/${name}.zip`;
  console.log(`downloading Node.js ${PINNED_NODE.version} for the journeys from ${url}`);
  const reply = await fetch(url);
  if (!reply.ok) throw new Error(`could not download ${url}: HTTP ${reply.status}`);
  const zip = Buffer.from(await reply.arrayBuffer());
  const sha256 = crypto.createHash('sha256').update(zip).digest('hex');
  if (sha256 !== PINNED_NODE.zipSha256) {
    throw new Error(`${url} has SHA-256 ${sha256}, not the expected ${PINNED_NODE.zipSha256}; nothing was installed`);
  }

  // Unpack beside the final folder and rename it into place, so an
  // interrupted unpack never leaves a half-filled folder that looks ready.
  const staging = `${home}.partial`;
  fs.rmSync(staging, { recursive: true, force: true });
  fs.mkdirSync(staging, { recursive: true });
  const zipFile = path.join(staging, `${name}.zip`);
  fs.writeFileSync(zipFile, zip);
  // Windows' own tar.exe (bsdtar) reads ZIP files; Git's GNU tar does not.
  const tar = path.join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'tar.exe');
  const unpacked = spawnSync(tar, ['-xf', zipFile, '-C', staging], { stdio: 'inherit', windowsHide: true });
  if (unpacked.status !== 0) throw new Error(`${tar} could not unpack ${zipFile} (exit ${unpacked.status})`);
  fs.rmSync(zipFile);
  fs.rmSync(home, { recursive: true, force: true });
  fs.renameSync(staging, home);
  return node;
}

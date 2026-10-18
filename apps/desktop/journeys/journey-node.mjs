// Gives the Windows journeys their own, pinned Node.js (kicad-parts-e24c,
// mulch mx-625975).
//
// Node.js before 24.16.0 on Windows, every 25.x and 26.0.x carry libuv
// #5106: on every TCP connect (and keepalive call) libuv asks Windows for
// its version with a struct whose size field was never set. Now and then
// Windows writes past it, the stack check trips, and the process ends with
// exit code 0xC0000409 and prints nothing. About 1 journey test process in
// 600 died this way at launchApp's request to tauri-driver, which then
// logged "os error 10054". libuv fixed it in aabb7651de; Node.js has the
// fix from 24.16.0 (nodejs/node#62561) and 26.1.0.
//
// We pin 24.22.0 in infra/versions.lock because it also has the fix for
// nodejs/node#56645 (#61999, in 24.20.0), a separate crash at exit that
// does print "Assertion failed". On Windows the runner always re-runs
// itself under the pinned Node.js, so every machine runs the same one.

import crypto from 'node:crypto';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

/** How Windows reports a fast-fail, including this crash. */
export const NODE_CRASH_EXIT_CODE = 0xc0000409;

/** How long the one-time download may take before we give up. */
const DOWNLOAD_TIMEOUT_MS = 10 * 60 * 1000;

/**
 * On Windows, makes sure this script runs under the pinned Node.js: if it
 * does not, downloads that Node.js once (the only network call made under
 * the running one), runs this script again under it with the same
 * arguments, and exits with its exit code. Elsewhere it does nothing.
 *
 * Call it first, before anything opens a TCP connection.
 */
export async function runUnderPinnedNode(repoRoot, script) {
  if (process.platform !== 'win32') return;
  const pin = readPin(repoRoot);
  const node = path.join(repoRoot, 'target', 'journey-tools', 'node', `v${pin.version}`, `node-v${pin.version}-win-x64`, 'node.exe');
  if (samePath(process.execPath, node)) return;

  const running = process.versions.node;
  if (hasLibuv5106(running)) {
    console.log(
      `Your Node.js ${running} crashes now and then on Windows (libuv #5106), so the journeys use their own ` +
        `Node.js ${pin.version}. Upgrade your Node.js to 24.16.0 or newer to protect npm and other tools too.`,
    );
  }
  try {
    await installPinnedNode(pin, node);
  } catch (error) {
    const cause = error.cause ? ` (${error.cause.message ?? error.cause})` : '';
    console.error(
      `could not get Node.js ${pin.version} for the journeys: ${error.message}${cause}. ` +
        `The journeys download Node.js ${pin.version} once on Windows. Connect to the internet and run again.`,
    );
    process.exit(1);
  }
  // Ctrl+C stops both runners, as they share the console. If this one is
  // killed with taskkill /F, the re-run one still finishes and cleans up.
  const rerun = spawnSync(node, [script, ...process.argv.slice(2)], { stdio: 'inherit' });
  if (rerun.error) {
    console.error(`could not start ${node}: ${rerun.error.message}`);
    process.exit(1);
  }
  process.exit(rerun.status ?? 1);
}

/** The Node.js running now, as recorded in evidence and printed in the log. */
export function currentNode() {
  return { version: process.versions.node, path: process.execPath };
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

/** Reads toolchain.node_windows_x64_url and _sha256 from infra/versions.lock. */
function readPin(repoRoot) {
  const lockFile = path.join(repoRoot, 'infra', 'versions.lock');
  const text = fs.readFileSync(lockFile, 'utf8');
  const value = (key) => text.match(new RegExp(`^${key} = "([^"]+)"$`, 'm'))?.[1];
  const url = value('node_windows_x64_url');
  const sha256 = value('node_windows_x64_sha256');
  const version = url?.match(/\/dist\/v(\d+\.\d+\.\d+)\//)?.[1];
  if (!url || !sha256 || !version) {
    throw new Error(`${lockFile} needs toolchain.node_windows_x64_url and node_windows_x64_sha256`);
  }
  return { url, sha256, version };
}

/** True for the Node.js versions that still carry libuv #5106. */
function hasLibuv5106(version) {
  const [major, minor] = version.split('.').map(Number);
  return (major === 24 && minor < 16) || major === 25 || (major === 26 && minor < 1);
}

function samePath(a, b) {
  return path.resolve(a).toLowerCase() === path.resolve(b).toLowerCase();
}

/**
 * Downloads, checks and unpacks the pinned Node.js so that `node` exists.
 * Safe when two runs do it at once: each unpacks into its own folder, and
 * a finished folder is never replaced or deleted.
 */
async function installPinnedNode(pin, node) {
  const home = path.dirname(path.dirname(node));
  if (isComplete(node, pin)) return;

  console.log(`downloading Node.js ${pin.version} for the journeys from ${pin.url}`);
  const reply = await fetch(pin.url, { signal: AbortSignal.timeout(DOWNLOAD_TIMEOUT_MS) });
  if (!reply.ok) throw new Error(`${pin.url} answered HTTP ${reply.status}`);
  const zip = Buffer.from(await reply.arrayBuffer());
  const sha256 = crypto.createHash('sha256').update(zip).digest('hex');
  if (sha256 !== pin.sha256) {
    throw new Error(`${pin.url} has SHA-256 ${sha256}, not ${pin.sha256} as infra/versions.lock says; nothing was installed`);
  }

  const staging = `${home}.partial-${process.pid}-${crypto.randomBytes(4).toString('hex')}`;
  try {
    fs.mkdirSync(staging, { recursive: true });
    const zipFile = path.join(staging, path.basename(pin.url));
    fs.writeFileSync(zipFile, zip);
    // Windows' own tar.exe (bsdtar) reads ZIP files; Git's GNU tar does not.
    const tar = path.join(process.env.SystemRoot ?? 'C:\\Windows', 'System32', 'tar.exe');
    const unpacked = spawnSync(tar, ['-xf', zipFile, '-C', staging], { stdio: 'inherit', windowsHide: true });
    if (unpacked.status !== 0) throw new Error(`${tar} could not unpack ${zipFile} (exit ${unpacked.status})`);
    fs.rmSync(zipFile);
    fs.writeFileSync(path.join(staging, '.complete'), `${pin.sha256}\n`);
    try {
      fs.renameSync(staging, home);
    } catch (error) {
      if (isComplete(node, pin)) return; // another run finished first; keep its copy
      if (!fs.existsSync(home)) throw error;
      // An unfinished folder left by an older runner or a crash. No run
      // uses it, because runs only use a folder with a matching .complete.
      fs.rmSync(home, { recursive: true, force: true });
      fs.renameSync(staging, home);
    }
  } finally {
    fs.rmSync(staging, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  }
}

/** True when `node` is the pinned Node.js, unpacked in full and checked. */
function isComplete(node, pin) {
  const home = path.dirname(path.dirname(node));
  try {
    return fs.existsSync(node) && fs.readFileSync(path.join(home, '.complete'), 'utf8').trim() === pin.sha256;
  } catch {
    return false;
  }
}

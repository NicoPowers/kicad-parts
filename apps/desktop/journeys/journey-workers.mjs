// Runs journey specs side by side. Each worker has its own tauri-driver
// (inside its own journey-desktop, so its own hidden desktop) on its own
// free ports, and runs one spec at a time. Each spec writes its evidence to
// its own folder, by-spec/<spec>/, which mergeSpecOutputs then merges into
// the run's folder in spec order, as if one process had run them all.
//
// What keeps specs apart when they run at the same time:
// - tauri-driver and msedgedriver ports: free ports per worker.
// - hidden desktop: journey-desktop names it after its own process ID.
// - temporary folders (data dirs, fake kicad-cli copies, library copies):
//   harness.mjs names them after the spec's number, which no two specs
//   share (checked in checkSpecNumbers).
// - team servers: catalog-import picks a free port and its own data dir.
// - evidence: each spec has its own JOURNEY_OUT until the merge.

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';

/** The files every spec appends to; the merge joins them in spec order. */
const JOINED_FILES = new Set(['evidence.jsonl', 'visual-results.jsonl']);

/**
 * Stops the run when two specs start with the same number: they would
 * share temporary folders (harness.mjs `tempDir`) if they ran together.
 */
export function checkSpecNumbers(specNames) {
  const byNumber = new Map();
  for (const name of specNames) {
    const number = name.slice(0, 2);
    byNumber.set(number, [...(byNumber.get(number) ?? []), name]);
  }
  const clashes = [...byNumber.values()].filter((names) => names.length > 1);
  if (clashes.length > 0) {
    throw new Error(
      `These specs share a number, so they would share temporary folders: ${clashes.map((c) => c.join(' and ')).join('; ')}. ` +
        'Give each spec its own two-digit number.',
    );
  }
}

/** A port on 127.0.0.1 that was free a moment ago. */
export function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

function waitForPort(port, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    const attempt = () => {
      const socket = net.connect(port, '127.0.0.1');
      socket.once('connect', () => {
        socket.end();
        resolve();
      });
      socket.once('error', () => {
        socket.destroy();
        if (Date.now() > deadline) reject(new Error(`port ${port} never opened`));
        else setTimeout(attempt, 200);
      });
    };
    attempt();
  });
}

/**
 * Starts one worker's tauri-driver and waits until it answers.
 * Killing journey-desktop closes its job, which ends tauri-driver and
 * everything it started. journey-desktop also ends everything if this
 * runner is killed outright, because it watches its parent process.
 * @returns {Promise<{ id: number, port: number, stop: () => void }>}
 */
export async function startWorker(id, { visible, journeyDesktop, nativeDriver }) {
  const port = await freePort();
  let nativePort = await freePort();
  while (nativePort === port) nativePort = await freePort();
  const driverArgs = ['--port', String(port), '--native-port', String(nativePort), '--native-driver', nativeDriver];
  const driver = visible
    ? spawn('tauri-driver', driverArgs, { stdio: ['ignore', 'inherit', 'inherit'] })
    : spawn(journeyDesktop, ['tauri-driver', ...driverArgs], { stdio: ['ignore', 'inherit', 'inherit'] });
  const failed = new Promise((_, reject) =>
    driver.once('error', (error) => {
      const what = visible ? 'tauri-driver (cargo install tauri-driver --locked)' : `journey-desktop (${journeyDesktop})`;
      reject(new Error(`could not start ${what}: ${error.message}`));
    }),
  );
  await Promise.race([waitForPort(port, 20_000), failed]);
  return { id, port, stop: () => driver.kill() };
}

/**
 * Runs `specs` on `workers`: each worker takes the next spec from the
 * queue when it finishes one. Resolves when every spec has run.
 * @param {(worker: object, spec: string) => Promise<void>} runSpec
 */
export async function runQueue(workers, specs, runSpec) {
  const queue = [...specs];
  await Promise.all(
    workers.map(async (worker) => {
      for (let spec = queue.shift(); spec; spec = queue.shift()) await runSpec(worker, spec);
    }),
  );
}

/**
 * Runs one spec file with node --test on `worker`'s driver. Its evidence
 * goes to `specOut`, its JUnit result to specOut/junit.xml and its console
 * output to specOut/output.txt. With `live`, the output also streams to
 * this console as it comes (one worker); otherwise it prints in one block
 * when the spec ends, so specs running side by side do not mix lines.
 * @returns {Promise<{ status: number, seconds: number }>}
 */
export function runSpecFile({ testNode, spec, specOut, worker, cwd, env, live }) {
  fs.rmSync(specOut, { recursive: true, force: true });
  fs.mkdirSync(specOut, { recursive: true });
  const started = Date.now();
  const child = spawn(
    testNode.path,
    [
      '--test',
      // Once its tests and their after() hooks are done, a spec ends at
      // once. Without this, the team server's process handle held each
      // spec's process open for about 15 s more (TEST_TIMES.md).
      '--test-force-exit',
      '--test-reporter=spec',
      '--test-reporter-destination=stdout',
      '--test-reporter=junit',
      `--test-reporter-destination=${path.join(specOut, 'junit.xml')}`,
      spec,
    ],
    {
      cwd,
      stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...env, JOURNEY_OUT: specOut, TAURI_DRIVER_PORT: String(worker.port) },
    },
  );
  const chunks = [];
  const keep = (chunk) => {
    chunks.push(chunk);
    if (live) process.stdout.write(chunk);
  };
  child.stdout.on('data', keep);
  child.stderr.on('data', keep);
  return new Promise((resolve) => {
    child.once('close', (code) => {
      const output = Buffer.concat(chunks);
      fs.writeFileSync(path.join(specOut, 'output.txt'), output);
      const seconds = (Date.now() - started) / 1000;
      const name = path.basename(spec);
      if (!live) {
        console.log(`\n=== ${name} (worker ${worker.id}, ${seconds.toFixed(1)} s) ===`);
        process.stdout.write(output);
      }
      resolve({ status: code ?? 1, seconds });
    });
    child.once('error', (error) => {
      fs.writeFileSync(path.join(specOut, 'output.txt'), `could not start ${testNode.path}: ${error.message}\n`);
      resolve({ status: 1, seconds: 0 });
    });
  });
}

/**
 * Moves every spec's evidence from bySpecDir/<spec>/ into outDir, in the
 * order of `specStems`: joined files are appended, JUnit results become
 * one junit.xml, console output goes to logs/<spec>.txt, and every other
 * file (screenshots, accessibility results, images of the differences,
 * and so on) moves to the same place under outDir. Two specs writing the
 * same file is a mistake, so it stops the run.
 */
export function mergeSpecOutputs(bySpecDir, specStems, outDir) {
  const suites = [];
  for (const stem of specStems) {
    const specOut = path.join(bySpecDir, stem);
    if (!fs.existsSync(specOut)) continue;
    for (const relative of filesUnder(specOut)) {
      const from = path.join(specOut, relative);
      if (relative === 'junit.xml') {
        suites.push(junitSuites(fs.readFileSync(from, 'utf8')));
      } else if (relative === 'output.txt') {
        moveFile(from, path.join(outDir, 'logs', `${stem}.txt`));
      } else if (JOINED_FILES.has(relative)) {
        fs.appendFileSync(path.join(outDir, relative), fs.readFileSync(from));
      } else {
        const to = path.join(outDir, relative);
        if (fs.existsSync(to)) throw new Error(`two specs wrote ${relative}; ${stem} is the second`);
        moveFile(from, to);
      }
    }
  }
  fs.writeFileSync(
    path.join(outDir, 'junit.xml'),
    `<?xml version="1.0" encoding="utf-8"?>\n<testsuites>\n${suites.join('\n')}\n</testsuites>\n`,
  );
  fs.rmSync(bySpecDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
}

/** What is inside <testsuites> of one node --test JUnit file. */
function junitSuites(xml) {
  const start = xml.indexOf('<testsuites>');
  const end = xml.lastIndexOf('</testsuites>');
  if (start < 0) return xml.replace(/^<\?xml[^>]*\?>\s*/, '').trim();
  return xml.slice(start + '<testsuites>'.length, end < 0 ? undefined : end).trim();
}

function filesUnder(dir, prefix = '') {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const relative = path.join(prefix, entry.name);
    return entry.isDirectory() ? filesUnder(path.join(dir, entry.name), relative) : [relative];
  });
}

function moveFile(from, to) {
  fs.mkdirSync(path.dirname(to), { recursive: true });
  fs.renameSync(from, to);
}

/**
 * Counts tests in a merged junit.xml: node --test writes one <testcase>
 * per test, with a <failure> or <skipped> inside when it did not pass.
 * Suites (describe blocks) are <testsuite>s and are not counted.
 */
export function countTests(junitXml) {
  const cases = junitXml.match(/<testcase\b[^>]*?(\/>|>[\s\S]*?<\/testcase>)/g) ?? [];
  const failed = cases.filter((c) => c.includes('<failure')).length;
  const skipped = cases.filter((c) => c.includes('<skipped')).length;
  return { tests: cases.length, passed: cases.length - failed - skipped, failed, skipped };
}

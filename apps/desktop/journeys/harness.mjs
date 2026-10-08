// Shared helpers for the native desktop journeys. Each journey launches the
// real Tauri app (WebView2) through tauri-driver, so commands, kicad-cli and
// the preview cache are all real.

import { remote } from 'webdriverio';
import axe from 'axe-core';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..');
export const outDir = process.env.JOURNEY_OUT ?? path.join(repoRoot, 'target', 'journeys', 'latest');
export const appBinary = path.join(
  repoRoot,
  'target',
  'debug',
  process.platform === 'win32' ? 'kicad-parts-desktop.exe' : 'kicad-parts-desktop',
);
export const driverPort = Number(process.env.TAURI_DRIVER_PORT ?? 4444);

/** The viewport every screenshot is taken at (the window's configured inner size). */
export const VIEWPORT = { width: 1280, height: 800 };

const screenshotsDir = path.join(outDir, 'screenshots');
const a11yDir = path.join(outDir, 'accessibility');
const evidenceFile = path.join(outDir, 'evidence.jsonl');

export function record(entry) {
  fs.mkdirSync(outDir, { recursive: true });
  fs.appendFileSync(evidenceFile, `${JSON.stringify({ at: new Date().toISOString(), ...entry })}\n`);
}

export function tempDir(prefix) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `kicad-parts-${prefix}-`));
}

/**
 * A stand-in kicad-cli (built from tooling/fake-kicad-cli) copied into a
 * temporary folder as kicad-cli.exe. `version` prints `version`; with
 * `null` it fails like a damaged install.
 */
export function fakeKicadCli(version) {
  const built = path.join(
    repoRoot,
    'target',
    'debug',
    process.platform === 'win32' ? 'fake-kicad-cli.exe' : 'fake-kicad-cli',
  );
  assert.ok(fs.existsSync(built), `build the fake first: ${built}`);
  const dir = tempDir('fake-kicad');
  const cli = path.join(dir, process.platform === 'win32' ? 'kicad-cli.exe' : 'kicad-cli');
  fs.copyFileSync(built, cli);
  if (version !== null) fs.writeFileSync(path.join(dir, 'fake-version.txt'), version);
  return cli;
}

/** A data directory whose saved settings already point at `cli`. */
export function dataDirWithOverride(cli) {
  const dataDir = tempDir('data');
  fs.mkdirSync(path.join(dataDir, 'config'), { recursive: true });
  fs.writeFileSync(
    path.join(dataDir, 'config', 'settings.json'),
    JSON.stringify({ kicad_cli_override: cli }),
  );
  return dataDir;
}

/** Path of the installed KiCad 10 kicad-cli (KICAD_CLI overrides). */
export function installedKicadCli() {
  if (process.env.KICAD_CLI) return process.env.KICAD_CLI;
  const roots = [process.env.ProgramFiles, process.env.ProgramW6432].filter(Boolean);
  for (const root of roots) {
    const kicad = path.join(root, 'KiCad');
    if (!fs.existsSync(kicad)) continue;
    const versions = fs.readdirSync(kicad).filter((v) => v.startsWith('10.')).sort().reverse();
    for (const version of versions) {
      const cli = path.join(kicad, version, 'bin', 'kicad-cli.exe');
      if (fs.existsSync(cli)) return cli;
    }
  }
  throw new Error('KiCad 10 is required for the journeys; install it or set KICAD_CLI.');
}

/**
 * Launches the desktop app with isolated settings and cache.
 * @param {{ args?: string[], dataDir?: string }} options
 */
export async function launchApp({ args = [], dataDir = tempDir('data') } = {}) {
  assert.ok(fs.existsSync(appBinary), `build the app first: ${appBinary}`);
  const browser = await remote({
    hostname: '127.0.0.1',
    port: driverPort,
    logLevel: 'warn',
    connectionRetryTimeout: 120_000,
    capabilities: {
      'tauri:options': {
        application: appBinary,
        // msedgedriver treats these as browser switches: use --name=value.
        args: [`--data-dir=${dataDir}`, ...args],
      },
    },
  });
  const app = { browser, dataDir };
  await browser.waitUntil(
    async () => (await browser.execute(() => document.querySelector('.topbar') !== null)) === true,
    { timeout: 30_000, timeoutMsg: 'the Leptos UI did not mount' },
  );
  const viewport = await browser.execute(() => ({
    width: window.innerWidth,
    height: window.innerHeight,
    devicePixelRatio: window.devicePixelRatio,
  }));
  record({ event: 'launch', args, viewport });
  assert.equal(viewport.width, VIEWPORT.width, 'stable viewport width');
  assert.equal(viewport.height, VIEWPORT.height, 'stable viewport height');
  return app;
}

/** Saves a screenshot of the current state after layout settles. */
export async function screenshot(browser, name) {
  fs.mkdirSync(screenshotsDir, { recursive: true });
  // Remove the text caret and focus ring so repeated runs match.
  await browser.execute(() => {
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
  });
  await browser.execute(
    () =>
      new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve(true)))),
  );
  const file = path.join(screenshotsDir, `${name}.png`);
  await browser.saveScreenshot(file);
  record({ event: 'screenshot', name, file });
  return file;
}

/**
 * Runs axe-core against the live document, writes the full result, and
 * fails on any violation.
 */
export async function assertAccessible(browser, name) {
  fs.mkdirSync(a11yDir, { recursive: true });
  await browser.execute(axe.source);
  const results = await browser.executeAsync((done) => {
    // eslint-disable-next-line no-undef
    window.axe
      .run(document, { resultTypes: ['violations', 'incomplete'] })
      .then((r) =>
        done({
          violations: r.violations.map((v) => ({
            id: v.id,
            impact: v.impact,
            help: v.help,
            nodes: v.nodes.map((n) => n.target.join(' ')),
          })),
          incomplete: r.incomplete.map((v) => ({ id: v.id, nodes: v.nodes.length })),
          passes: r.passes.length,
        }),
      )
      .catch((error) => done({ error: String(error) }));
  });
  fs.writeFileSync(path.join(a11yDir, `${name}.json`), JSON.stringify(results, null, 2));
  record({ event: 'axe', name, violations: results.violations?.length, passes: results.passes });
  assert.equal(results.error, undefined, `axe failed to run: ${results.error}`);
  assert.deepEqual(results.violations, [], `accessibility violations in ${name}`);
  return results;
}

export async function textOf(browser, selector) {
  return browser.execute((s) => document.querySelector(s)?.textContent?.trim() ?? null, selector);
}

export async function waitForText(browser, selector, expected, timeout = 20_000) {
  const matches = (text) =>
    text !== null && (expected instanceof RegExp ? expected.test(text) : text === expected);
  await browser.waitUntil(async () => matches(await textOf(browser, selector)), {
    timeout,
    interval: 50,
    timeoutMsg: `${selector} never showed ${expected}; last: ${JSON.stringify(await textOf(browser, selector))}`,
  });
}

/** Describes every image in `selector`, including whether it decoded. */
export async function images(browser, selector) {
  return browser.execute(
    (s) =>
      [...document.querySelectorAll(s)].map((img) => ({
        alt: img.alt,
        loaded: img.complete && img.naturalWidth > 0,
        width: img.naturalWidth,
        svg: img.src.startsWith('data:image/svg+xml;base64,'),
      })),
    selector,
  );
}

export async function searchFor(browser, query, expectedCount) {
  const input = await browser.$('aria/Search parts');
  assert.equal(await input.getTagName(), 'input', 'the search box is labelled "Search parts"');
  const started = Date.now();
  await input.setValue(query);
  // Wait until the list shows the answer to this query, not the last one.
  await browser.waitUntil(
    async () =>
      (await browser.execute(
        () => document.querySelector('ul[aria-label="Search results"]')?.dataset.query ?? null,
      )) === query,
    { timeout: 20_000, interval: 25, timeoutMsg: `results for ${query} never arrived` },
  );
  await waitForText(browser, '#result-count', expectedCount);
  const elapsed = Date.now() - started;
  record({ event: 'search', query, expectedCount, elapsedMs: elapsed });
  return elapsed;
}

export async function selectResult(browser, name) {
  const buttons = await browser.$$('ul[aria-label="Search results"] li button');
  for (const button of buttons) {
    if ((await button.getText()).includes(name)) {
      await button.click();
      await browser.waitUntil(async () => (await button.getAttribute('aria-current')) === 'true', {
        timeout: 5_000,
        timeoutMsg: `${name} was not marked as the current result`,
      });
      return;
    }
  }
  assert.fail(`no search result mentions ${name}`);
}

export const SYMBOL = 'section[aria-labelledby="symbol-heading"]';
export const FOOTPRINT = 'section[aria-labelledby="footprint-heading"]';

export async function waitForPreview(browser, section, timeout = 60_000) {
  await browser.waitUntil(
    async () =>
      browser.execute(
        (s) => {
          const el = document.querySelector(s);
          if (!el || el.querySelector('[aria-busy="true"]')) return false;
          const imgs = [...el.querySelectorAll('img')];
          return el.querySelector('.problem') !== null || (imgs.length > 0 && imgs.every((i) => i.complete));
        },
        section,
      ),
    { timeout, interval: 100, timeoutMsg: `${section} did not finish loading` },
  );
}

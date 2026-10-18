// Shared helpers for the native desktop journeys. Each journey launches the
// real Tauri app (WebView2) through tauri-driver, so commands, kicad-cli and
// the preview cache are all real.

import { remote } from 'webdriverio';
import { spawn } from 'node:child_process';
import axe from 'axe-core';
import pixelmatch from 'pixelmatch';
import { PNG } from 'pngjs';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
export const repoRoot = path.resolve(here, '../../..');
export const outDir = process.env.JOURNEY_OUT ?? path.join(repoRoot, 'target', 'journeys', 'latest');
export const appBinary = path.join(
  repoRoot,
  'target',
  'debug',
  process.platform === 'win32' ? 'kicad-parts-desktop.exe' : 'kicad-parts-desktop',
);
export const driverPort = Number(process.env.TAURI_DRIVER_PORT ?? 4444);

/** The viewport every screenshot is taken at (the window's configured inner
 * size). The app runs with --display-scale=1, so one CSS pixel is one
 * screenshot pixel whatever the Windows display scaling is. */
export const VIEWPORT = { width: 1280, height: 800 };

/** Reviewed screenshots that each new screenshot is compared with. */
export const baselinesDir = path.join(here, 'baselines');

/**
 * How different a screenshot may be from its approved screenshot and still
 * pass. A pixel counts as different when its colour moved more than
 * PIXEL_THRESHOLD (pixelmatch's 0-1 colour distance), anti-aliased edges
 * included. The screenshot fails when more than MAX_DIFFERENT_PIXELS
 * differ. Repeated runs differ by 0 pixels: --software-rendering draws
 * without the GPU and in whole tiles, since a sleeping or reset GPU, or a
 * partly redrawn tile, changes edge smoothing.
 * Journey 01 checks that small real changes (one character, one colour
 * token, the corner radius) fail.
 */
export const VISUAL_LIMITS = { PIXEL_THRESHOLD: 0.02, MAX_DIFFERENT_PIXELS: 4 };

export const screenshotsDir = path.join(outDir, 'screenshots');
const differencesDir = path.join(outDir, 'differences');
/** Where --update-baselines puts new screenshots until the runner accepts them. */
export const stagedBaselinesDir = path.join(outDir, 'new-baselines');
const a11yDir = path.join(outDir, 'accessibility');
const evidenceFile = path.join(outDir, 'evidence.jsonl');
/** One line per screenshot: how it compared with its approved screenshot. */
export const visualResultsFile = path.join(outDir, 'visual-results.jsonl');
const updateBaselines = process.env.JOURNEYS_UPDATE_BASELINES === '1';

export function record(entry) {
  fs.mkdirSync(outDir, { recursive: true });
  fs.appendFileSync(evidenceFile, `${JSON.stringify({ at: new Date().toISOString(), ...entry })}\n`);
}

/** Temporary files of one run. The runner empties it before and after. */
export const runTempRoot = path.join(os.tmpdir(), 'kicad-parts-journeys');
const specName = path.basename(process.argv.find((arg) => arg.endsWith('.test.mjs')) ?? 'journey');
const tempNamesUsed = new Map();

/**
 * A new, empty folder such as <temp>/kicad-parts-journeys/02-no-kicad.
 * The name is fixed rather than random, so paths the app shows look the
 * same in every run's screenshots.
 */
export function tempDir(prefix) {
  const base = `${specName.slice(0, 2)}-${prefix}`;
  const count = (tempNamesUsed.get(base) ?? 0) + 1;
  tempNamesUsed.set(base, count);
  const dir = path.join(runTempRoot, count === 1 ? base : `${base}-${count}`);
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  return dir;
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
  // The Libraries browser reads KiCad's settings folder for the person's
  // library tables; journeys never read the developer's own, so each
  // launch gets an empty folder unless the journey passes fixture tables.
  if (!args.some((arg) => arg.startsWith('--kicad-config-dir='))) {
    args = [...args, `--kicad-config-dir=${tempDir('kicad-config')}`];
  }
  const browser = await remote({
    hostname: '127.0.0.1',
    port: driverPort,
    logLevel: 'warn',
    connectionRetryTimeout: 120_000,
    capabilities: {
      'tauri:options': {
        application: appBinary,
        // msedgedriver treats these as browser switches: use --name=value.
        // --display-scale=1 and --software-rendering make every screenshot
        // 1280x800 and drawn the same way, whatever the display scaling or
        // the GPU's state.
        args: [`--data-dir=${dataDir}`, '--display-scale=1', '--software-rendering', ...args],
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
  assert.equal(viewport.devicePixelRatio, 1, 'the display scale is pinned to 1');
  return app;
}

/**
 * Saves a screenshot of the current state after layout settles, then
 * compares it with its approved screenshot in baselines/ (see
 * compareWithBaseline). A screenshot that does not match fails the test
 * at this point, so the failure shows in the test results.
 *
 * `blankOut` lists selectors whose text changes from run to run, such as a
 * server port; times (<time>) are always blanked out, with every digit
 * shown as 0 first (see capture). Their text is made transparent and
 * hatched for the screenshot only, so the layout stays real.
 * @param {{ blankOut?: string[] }} options
 */
export async function screenshot(browser, name, { blankOut = [] } = {}) {
  const file = path.join(screenshotsDir, `${name}.png`);
  const blankedOut = await capture(browser, file, blankOut);
  record({ event: 'screenshot', name, file, blankedOut });
  const result = compareWithBaseline(name, file);
  assert.ok(result.status === 'same' || result.status === 'staged', visualProblem(result));
  return file;
}

/**
 * Takes a screenshot into `file` with the caret and focus ring removed and
 * `blankOut` (plus every <time>) blanked out; returns how many elements
 * were blanked out.
 */
export async function capture(browser, file, blankOut = []) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  // Remove the text caret and focus ring so repeated runs match.
  await browser.execute(() => {
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
  });
  // Clock times must not change a screenshot. Blanking a time still leaves
  // its width, which moves with its digits by a fraction of a pixel, enough
  // to shift the full stop after it. So every digit of a <time> shows as 0
  // for the screenshot: the same text every run, so the same width.
  await showTimeDigitsAs(browser, '0', CAPTURE_TIMES);
  try {
    const blankedOut = await browser.execute((selectors) => {
      const elements = selectors.flatMap((s) => [...document.querySelectorAll(s)]);
      for (const el of elements) {
        // CSSOM, not a style attribute or <style> element, which the CSP blocks.
        el.style.setProperty('color', 'transparent', 'important');
        el.style.setProperty('-webkit-text-fill-color', 'transparent', 'important');
        el.style.setProperty(
          'background-image',
          'repeating-linear-gradient(135deg, #98a2b3 0 1px, transparent 1px 5px)',
          'important',
        );
        el.dataset.journeyBlankedOut = '';
      }
      return elements.length;
    }, ['time', ...blankOut]);
    await browser.execute(
      () =>
        new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve(true)))),
    );
    await browser.saveScreenshot(file);
    return blankedOut;
  } finally {
    // Always undo, even if the screenshot failed: a later test must see
    // the real text and colours, or it fails for a confusing reason.
    await browser.execute(() => {
      for (const el of document.querySelectorAll('[data-journey-blanked-out]')) {
        for (const property of ['color', '-webkit-text-fill-color', 'background-image']) el.style.removeProperty(property);
        delete el.dataset.journeyBlankedOut;
      }
    });
    await restoreTimes(browser, CAPTURE_TIMES);
  }
}

/** Where capture keeps the real text of the times it changes. */
const CAPTURE_TIMES = 'journeyCaptureTimes';

/**
 * Shows every digit of every <time> as `digit`, keeping the real text in
 * `window[key]` for restoreTimes. It changes each text node in place, so
 * the app keeps the nodes it owns, and the text keeps its length.
 */
export async function showTimeDigitsAs(browser, digit, key) {
  await browser.execute(
    (shownDigit, savedKey) => {
      const saved = [];
      for (const time of document.querySelectorAll('time')) {
        const walker = document.createTreeWalker(time, NodeFilter.SHOW_TEXT);
        for (let node = walker.nextNode(); node; node = walker.nextNode()) {
          const shown = node.data.replace(/\d/g, shownDigit);
          saved.push({ node, real: node.data, shown });
          node.data = shown;
        }
      }
      window[savedKey] = saved;
    },
    digit,
    key,
  );
}

/** Puts back the times showTimeDigitsAs changed, unless the app changed one meanwhile. */
export async function restoreTimes(browser, key) {
  await browser.execute((savedKey) => {
    for (const { node, real, shown } of window[savedKey] ?? []) {
      if (node.data === shown) node.data = real;
    }
    delete window[savedKey];
  }, key);
}

/**
 * Counts the pixels that differ between two PNG files and, if any differ,
 * writes an image of the differences to `differencesFile`.
 * @returns {{ differentPixels: number, sizeProblem?: string }}
 */
export function comparePngs(expectedFile, actualFile, differencesFile) {
  const expected = PNG.sync.read(fs.readFileSync(expectedFile));
  const actual = PNG.sync.read(fs.readFileSync(actualFile));
  if (expected.width !== actual.width || expected.height !== actual.height) {
    return { differentPixels: 0, sizeProblem: `the approved screenshot is ${expected.width}x${expected.height}` };
  }
  const differences = new PNG({ width: actual.width, height: actual.height });
  const differentPixels = pixelmatch(expected.data, actual.data, differences.data, actual.width, actual.height, {
    threshold: VISUAL_LIMITS.PIXEL_THRESHOLD,
    includeAA: true,
  });
  if (differentPixels > 0 && differencesFile) {
    fs.mkdirSync(path.dirname(differencesFile), { recursive: true });
    fs.writeFileSync(differencesFile, smallPng(differences));
  }
  return { differentPixels };
}

/**
 * Compares a screenshot with baselines/<name>.png and appends the outcome
 * to visual-results.jsonl, which the runner summarises. A screenshot that
 * differs keeps an image of the differences in differences/. With
 * --update-baselines the screenshot is staged in new-baselines/ instead;
 * the runner copies staged screenshots into baselines/ only if every
 * journey passed, which is the only way an approved screenshot changes.
 */
function compareWithBaseline(name, file) {
  const actual = PNG.sync.read(fs.readFileSync(file));
  assert.deepEqual(
    { width: actual.width, height: actual.height },
    VIEWPORT,
    `screenshot ${name} is ${actual.width}x${actual.height}, not ${VIEWPORT.width}x${VIEWPORT.height}: is the display scale pinned?`,
  );
  const baseline = path.join(baselinesDir, `${name}.png`);
  const result = { name, status: 'same', differentPixels: 0 };
  if (updateBaselines) {
    fs.mkdirSync(stagedBaselinesDir, { recursive: true });
    fs.writeFileSync(path.join(stagedBaselinesDir, `${name}.png`), smallPng(actual));
    result.status = 'staged';
  } else if (!fs.existsSync(baseline)) {
    result.status = 'missing';
  } else {
    const differencesFile = path.join(differencesDir, `${name}.png`);
    const { differentPixels, sizeProblem } = comparePngs(baseline, file, differencesFile);
    result.differentPixels = differentPixels;
    if (differentPixels > 0) result.differences = differencesFile;
    if (sizeProblem) {
      result.status = 'changed';
      result.reason = sizeProblem;
    } else if (differentPixels > VISUAL_LIMITS.MAX_DIFFERENT_PIXELS) {
      result.status = 'changed';
    }
  }
  fs.appendFileSync(visualResultsFile, `${JSON.stringify(result)}\n`);
  return result;
}

/** A plain message for a screenshot that is missing or changed. */
export function visualProblem(result) {
  const { name } = result;
  if (result.status === 'missing') {
    return (
      `${name} has no approved screenshot (baselines/${name}.png). Look at screenshots/${name}.png; ` +
      'if it is right, accept it with `npm run journeys -- --update-baselines`.'
    );
  }
  if (result.reason) return `${name} does not match its approved screenshot: ${result.reason}.`;
  const percent = ((100 * result.differentPixels) / (VIEWPORT.width * VIEWPORT.height)).toFixed(4);
  return (
    `${name} does not match its approved screenshot: ${result.differentPixels} pixels (${percent}%) differ, ` +
    `more than the ${VISUAL_LIMITS.MAX_DIFFERENT_PIXELS} allowed. The image of the differences is ` +
    `differences/${name}.png. If the change is intended, look at it, then run ` +
    '`npm run journeys -- --update-baselines`.'
  );
}

/** Re-encodes a PNG with the strongest compression, to keep baselines small. */
function smallPng(png) {
  return PNG.sync.write(png, { deflateLevel: 9, filterType: -1 });
}

/**
 * Runs axe-core against the live document, writes the full result to
 * accessibility/<name>.json, and fails on any violation.
 *
 * axe leaves a colour contrast check incomplete when it cannot see the
 * text's background: the text is cut off by a scrolled list
 * (elmPartiallyObscured) or partly under another element, such as an open
 * menu (bgOverlap). Each such element is checked again where axe can
 * measure it, scrolled into view and with a solid cover made invisible,
 * then the page is put back. Text under a see-through cover stays
 * incomplete. The journey fails on any contrast
 * violation and on any contrast check that is still incomplete.
 *
 * `disabledRules` turns off axe rules that cannot work on this page, by
 * their ids; the caller must say why. Every other rule still runs.
 * @param {{ disabledRules?: string[] }} options
 */
export async function assertAccessible(browser, name, { disabledRules = [] } = {}) {
  fs.mkdirSync(a11yDir, { recursive: true });
  await browser.execute(axe.source);
  const results = await browser.executeAsync(async (disabled, done) => {
    try {
      // eslint-disable-next-line no-undef
      const axe = window.axe;
      const reasonsOf = (node) => [...node.any, ...node.all, ...node.none].map((c) => c.data?.messageKey ?? c.message);
      const describe = (el) => el.tagName.toLowerCase() + [...el.classList].map((c) => '.' + c).join('');
      // A drawing in a viewer (kicad-parts-34df) is a picture, hidden from
      // assistive technology; its text, such as a footprint's reference, is
      // part of the picture, as it is in kicad-cli's images.
      const first = await axe.run(
        { exclude: [['.viewer-svg']] },
        {
          resultTypes: ['violations', 'incomplete'],
          rules: Object.fromEntries(disabled.map((id) => [id, { enabled: false }])),
        },
      );
      // axe places an element in its grid by its box clipped to overflow:
      // hidden parents, then tests the unclipped box, so a drawing zoomed
      // past its viewer seems to cover text near the viewer. A drawing can
      // cover only text inside a viewer whose overflow is hidden.
      const viewerAreas = [...document.querySelectorAll('.viewer-area')];
      const clipped = viewerAreas.every((area) => getComputedStyle(area).overflow === 'hidden');
      const violations = first.violations.map((v) => ({
        id: v.id,
        impact: v.impact,
        help: v.help,
        nodes: v.nodes.map((n) => n.target.join(' ')),
      }));
      const contrastRechecks = [];
      const unchecked = first.incomplete.find((v) => v.id === 'color-contrast')?.nodes ?? [];
      for (const node of unchecked) {
        const target = node.target.join(' ');
        const el = document.querySelector(target);
        const recheck = { target, reasons: reasonsOf(node), scrolled: false, madeInvisible: [] };
        contrastRechecks.push(recheck);
        if (!el) {
          recheck.outcome = 'incomplete';
          recheck.stillIncompleteBecause = ['element not found again'];
          continue;
        }
        const scrolls = [];
        for (let a = el.parentElement; a; a = a.parentElement) scrolls.push([a, a.scrollTop, a.scrollLeft]);
        el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
        recheck.scrolled = scrolls.some(([a, top, left]) => a.scrollTop !== top || a.scrollLeft !== left);
        // Elements drawn over the text, at its middle and both ends. A cover
        // is made invisible only if it is solid: its outermost element (the
        // one holding the others, such as a menu panel) has a fully opaque
        // background and is not faded. Text seen through a see-through
        // cover is never accepted.
        const solid = (c) => {
          const background = getComputedStyle(c).backgroundColor;
          const alpha = background.startsWith('rgba(') ? Number(background.slice(5, -1).split(',')[3]) : 1;
          if (!background.startsWith('rgb') || alpha < 1) return false;
          for (let a = c; a; a = a.parentElement) if (Number(getComputedStyle(a).opacity) < 1) return false;
          return true;
        };
        const box = el.getBoundingClientRect();
        const y = box.top + box.height / 2;
        const covering = new Set();
        const seeThrough = new Set();
        const samples = [box.left + 1, box.left + box.width / 2, box.right - 1];
        // Any part of the text's box inside a viewer could be covered.
        const insideAViewer = viewerAreas.some((area) => {
          const r = area.getBoundingClientRect();
          return box.left < r.right && box.right > r.left && box.top < r.bottom && box.bottom > r.top;
        });
        if (!clipped) {
          recheck.outcome = 'incomplete';
          recheck.stillIncompleteBecause = ['a drawing viewer does not hide what overflows it'];
          continue;
        }
        // Outside every viewer, no drawing can cover the text.
        if (!insideAViewer) for (const svg of document.querySelectorAll('.viewer-svg')) covering.add(svg);
        // A modal dialog, such as a close-up, sits above the whole page, but
        // axe still sees the page under it: text with lines over different
        // page parts seems to overlap them (elmPartiallyObscuring). The page
        // outside the dialog, apart from the dialog's own ancestors, is
        // inert under the backdrop, so it cannot be the text's background,
        // as long as the dialog itself is solid, not see-through. An open
        // header menu's panel, such as the KiCad settings, is drawn over the
        // page the same way: under a solid panel, the page outside it cannot
        // be its text's background either.
        const modal = el.closest('dialog');
        const panel = el.closest('details[open] > .kicad-settings-body');
        const cover = modal?.matches(':modal') ? modal : panel;
        if (cover && solid(cover)) {
          for (let node = cover; node.parentElement; node = node.parentElement) {
            for (const sibling of node.parentElement.children) if (sibling !== node) covering.add(sibling);
          }
        }
        for (const x of samples) {
          const over = [];
          for (const top of document.elementsFromPoint(x, y)) {
            if (top === el || el.contains(top)) break;
            if (!top.contains(el)) over.push(top);
          }
          const outermost = over.filter((c) => !over.some((other) => other !== c && other.contains(c)));
          for (const c of outermost) if (!solid(c)) seeThrough.add(describe(c));
          for (const c of over) covering.add(c);
        }
        if (seeThrough.size > 0) {
          for (const [a, top, left] of scrolls) {
            a.scrollTop = top;
            a.scrollLeft = left;
          }
          recheck.outcome = 'incomplete';
          recheck.stillIncompleteBecause = [`under a see-through cover: ${[...seeThrough].join(', ')}`];
          continue;
        }
        const visibility = [...covering].map((c) => [c, c.style.getPropertyValue('visibility'), c.style.getPropertyPriority('visibility')]);
        for (const c of covering) c.style.setProperty('visibility', 'hidden', 'important');
        recheck.madeInvisible = [...covering].map(describe);
        let again;
        try {
          again = await axe.run(el, {
            runOnly: { type: 'rule', values: ['color-contrast'] },
            resultTypes: ['violations', 'incomplete', 'passes'],
          });
        } finally {
          for (const [c, value, priority] of visibility) {
            if (value) c.style.setProperty('visibility', value, priority);
            else c.style.removeProperty('visibility');
          }
          for (const [a, top, left] of scrolls) {
            a.scrollTop = top;
            a.scrollLeft = left;
          }
        }
        const nodesOf = (list) => list.flatMap((v) => v.nodes);
        if (nodesOf(again.violations).length > 0) {
          recheck.outcome = 'violation';
          violations.push({
            id: 'color-contrast',
            impact: again.violations[0].impact,
            help: again.violations[0].help,
            nodes: [target],
            found: 'on the second check',
          });
        } else if (nodesOf(again.incomplete).length > 0 || nodesOf(again.passes).length === 0) {
          recheck.outcome = 'incomplete';
          recheck.stillIncompleteBecause = nodesOf(again.incomplete).flatMap(reasonsOf);
        } else {
          recheck.outcome = 'passed';
        }
      }
      done({
        violations,
        // Checks axe could not finish on the first pass, with its reason
        // for each element; contrastRechecks says how each contrast one
        // ended.
        incomplete: first.incomplete.map((v) => ({
          id: v.id,
          nodes: v.nodes.map((n) => ({ target: n.target.join(' '), reasons: reasonsOf(n) })),
        })),
        contrastRechecks,
        passes: first.passes.length,
        disabledRules: disabled,
      });
    } catch (error) {
      done({ error: String(error) });
    }
  }, disabledRules);
  fs.writeFileSync(path.join(a11yDir, `${name}.json`), JSON.stringify(results, null, 2));
  const stillIncomplete = (results.contrastRechecks ?? []).filter((r) => r.outcome === 'incomplete');
  record({
    event: 'axe',
    name,
    violations: results.violations?.length,
    passes: results.passes,
    contrastRechecked: results.contrastRechecks?.length,
    contrastStillIncomplete: stillIncomplete.length,
  });
  assert.equal(results.error, undefined, `axe failed to run: ${results.error}`);
  assert.deepEqual(results.violations, [], `accessibility violations in ${name}`);
  assert.deepEqual(
    stillIncomplete.map((r) => ({ target: r.target, reasons: r.stillIncompleteBecause })),
    [],
    `axe could not check the colour contrast of these elements in ${name}`,
  );
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
  if (query === '') {
    // Clearing the field through WebDriver fires no input event, so the
    // text is deleted from the keyboard as a person would.
    await input.click();
    await browser.keys(['Control', 'a']);
    await browser.keys('Backspace');
  } else {
    await input.setValue(query);
  }
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
  // The parts' own buttons, not the category section headers.
  const buttons = await browser.$$('ul[aria-label="Search results"] button.result');
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

/**
 * Focuses the part button `selector` finds, presses Enter, and checks that
 * it is selected and still has focus once the list has redrawn.
 */
export async function expectFocusKeptOnEnter(browser, selector) {
  const ipn = await browser.execute((s) => {
    const button = document.querySelector(s);
    button.focus();
    return button.querySelector('.result-ipn').textContent;
  }, selector);
  await browser.keys('Enter');
  await browser.waitUntil(
    async () => browser.execute(() => document.activeElement?.getAttribute('aria-current') === 'true'),
    { timeout: 5_000, timeoutMsg: 'Enter did not select the part' },
  );
  // The search runs again for the selection (120 ms debounce, then IPC).
  await new Promise((resolve) => setTimeout(resolve, 600));
  const focused = await browser.execute(() => ({
    isPart: document.activeElement?.classList.contains('result') ?? false,
    ipn: document.activeElement?.querySelector('.result-ipn')?.textContent ?? null,
    selected: document.activeElement?.getAttribute('aria-current') ?? null,
  }));
  assert.deepEqual(focused, { isPart: true, ipn, selected: 'true' }, 'focus stays on the chosen part');
}

/**
 * Waits until the part list shows the part `text` (its IPN or MPN) as the
 * selected one, in the open section named `section`, scrolled into view
 * and not covered by its sticky section header.
 */
export async function waitForSelectedInList(browser, text, section, timeout = 10_000) {
  let seen;
  const shown = browser.waitUntil(
    async () => {
      seen = await browser.execute((wanted) => {
        const row = document.querySelector('.results button.result[aria-current="true"]');
        if (!row || !row.textContent.includes(wanted)) return { row: row?.textContent ?? null };
        const toggle = row.closest('.result-section').querySelector('.section-toggle');
        const list = document.querySelector('.results').getBoundingClientRect();
        const box = row.getBoundingClientRect();
        const header = toggle.getBoundingClientRect();
        return {
          section: toggle.querySelector('.section-name').textContent,
          expanded: toggle.getAttribute('aria-expanded'),
          inView: box.top >= list.top - 1 && box.bottom <= list.bottom + 1,
          clearOfHeader: box.top >= header.bottom - 0.5,
        };
      }, text);
      return seen.section === section && seen.expanded === 'true' && seen.inView && seen.clearOfHeader;
    },
    { timeout, interval: 50 },
  );
  await shown.catch(() => assert.fail(`${text} was not shown selected in ${section}: ${JSON.stringify(seen)}`));
}

/**
 * Waits until the team badge says live updates are connected, and then
 * until the library check (shown to people who can edit) is worked out for
 * what the app follows, so screenshots always show its line.
 */
export async function waitForLiveUpdates(browser) {
  await browser.waitUntil(
    async () =>
      browser.execute(() => document.querySelector('.team-settings > summary')?.classList.contains('kicad-badge-ready')),
    { timeout: 20_000, timeoutMsg: 'live updates never connected' },
  );
  await waitForLibraryCheck(browser);
}

/**
 * Waits until the library check is worked out for what the app follows
 * now (or is not shown, for someone who can only view or is signed out).
 */
export async function waitForLibraryCheck(browser) {
  await browser.waitUntil(
    async () =>
      browser.execute(() => document.querySelector('.search-pane')?.dataset.libraryCheck !== 'loading'),
    { timeout: 30_000, interval: 50, timeoutMsg: 'the library check was never worked out' },
  );
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
          // Pictures, and drawing viewers (kicad-parts-34df) outside a close-up.
          const imgs = [...el.querySelectorAll('img')];
          const viewers = [...el.querySelectorAll('.viewer')].filter((v) => !v.closest('dialog.zoom-dialog'));
          const drawn = imgs.length + viewers.length > 0 && imgs.every((i) => i.complete) && viewers.every((v) => v.dataset.ready === 'true');
          return el.querySelector('.problem') !== null || drawn;
        },
        section,
      ),
    { timeout, interval: 100, timeoutMsg: `${section} did not finish loading` },
  );
}

/**
 * Waits until `section` shows a drawn preview: no longer busy, no problem
 * card (such as "Sign in to see this symbol", left from before signing in)
 * and every image loaded. Use it where a problem card would be wrong;
 * waitForPreview also accepts a problem card as finished.
 */
export async function waitForPreviewImages(browser, section, timeout = 60_000) {
  // What the section shows when it gives up, so a failure says why.
  const shown = async () =>
    browser.execute((s) => document.querySelector(s)?.innerText.replace(/\s+/g, ' ').trim() ?? '(no section)', section);
  try {
    await waitForDrawnImages(browser, section, timeout);
  } catch (error) {
    throw new Error(`${error.message}; it shows: ${await shown()}`);
  }
}

async function waitForDrawnImages(browser, section, timeout) {
  await browser.waitUntil(
    async () =>
      browser.execute(
        (s) => {
          const el = document.querySelector(s);
          if (!el || el.querySelector('[aria-busy="true"]') || el.querySelector('.problem')) return false;
          const imgs = [...el.querySelectorAll('img')];
          const viewers = [...el.querySelectorAll('.viewer')].filter((v) => !v.closest('dialog.zoom-dialog'));
          return (
            imgs.length + viewers.length > 0 &&
            imgs.every((i) => i.complete && i.naturalWidth > 0) &&
            viewers.every((v) => v.dataset.ready === 'true')
          );
        },
        section,
      ),
    { timeout, interval: 100, timeoutMsg: `${section} never showed a drawn preview` },
  );
}

/**
 * Starts `catalog-import local-team-server --json`: a disposable PocketBase
 * on 127.0.0.1 with every CSV file imported (or only `csv`, such as
 * `['g-res']`, which starts faster) and three accounts. It runs
 * windowless in a kill-on-close job and stops when its input closes, so it
 * cannot outlive this process.
 * @returns {Promise<{ url: string, accounts: { name: string, email: string, password: string, canEditCatalog: boolean }[], stop: () => Promise<void> }>}
 */
export async function startTeamServer({ csv = [] } = {}) {
  const exe = path.join(
    repoRoot,
    'target',
    'debug',
    process.platform === 'win32' ? 'catalog-import.exe' : 'catalog-import',
  );
  assert.ok(fs.existsSync(exe), `build it first: ${exe}`);
  const args = ['local-team-server', '--json', ...(csv.length > 0 ? ['--csv', csv.join(',')] : [])];
  const child = spawn(exe, args, {
    cwd: repoRoot,
    stdio: ['pipe', 'pipe', 'inherit'],
    windowsHide: true,
  });
  const exited = new Promise((resolve) => child.once('exit', resolve));
  const line = await new Promise((resolve, reject) => {
    let buffered = '';
    const timer = setTimeout(() => reject(new Error('the team server did not start in time')), 180_000);
    child.stdout.on('data', (chunk) => {
      buffered += chunk.toString('utf8');
      const end = buffered.indexOf('\n');
      if (end >= 0) {
        clearTimeout(timer);
        resolve(buffered.slice(0, end));
      }
    });
    child.once('exit', (code) => {
      clearTimeout(timer);
      reject(new Error(`the team server exited with ${code}`));
    });
  });
  const info = JSON.parse(line);
  record({ event: 'team-server', url: info.url, csv, accounts: info.accounts.map((a) => a.email) });
  // Later lines answer `reimport` requests, one each, in order.
  let rest = '';
  const waiting = [];
  child.stdout.on('data', (chunk) => {
    rest += chunk.toString('utf8');
    for (let end = rest.indexOf('\n'); end >= 0; end = rest.indexOf('\n')) {
      const answer = rest.slice(0, end);
      rest = rest.slice(end + 1);
      if (answer.includes('"reimported"')) waiting.shift()?.(JSON.parse(answer).reimported);
    }
  });
  return {
    ...info,
    /**
     * Imports the resistors again, as an importer would after reading the
     * team catalog, and returns the outcome, such as `nothing_changed`.
     * With `folder`, imports them from the copy of the CSV catalog there.
     */
    async reimport(folder) {
      const answered = new Promise((resolve) => waiting.push(resolve));
      child.stdin.write(folder ? `reimport ${folder}\n` : 'reimport\n');
      const outcome = await answered;
      record({ event: 'reimport', outcome });
      return outcome;
    },
    async stop() {
      child.stdin.end();
      const timeout = new Promise((resolve) => setTimeout(() => resolve('timeout'), 15_000));
      if ((await Promise.race([exited, timeout])) === 'timeout') child.kill();
    },
  };
}

/** Signs in to the team server as `account`; returns the auth token. */
export async function teamSignIn(url, account) {
  const reply = await fetch(`${url}/api/collections/users/auth-with-password`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ identity: account.email, password: account.password }),
  });
  assert.equal(reply.status, 200, 'the second person signs in');
  return (await reply.json()).token;
}

/**
 * Publishes an edit of `ipn` as someone else, the way another desktop
 * would: read the current details and revision, change fields, and POST
 * /api/catalog/edits with only the section that changed (Symbol and
 * Footprint belong to the revision, every other field to the details).
 * With `mpn`, it also gives the main manufacturer part that MPN.
 * Returns the Change's number.
 * @param {Record<string, string>} values field name to new value
 * @param {{ mpn?: string }} options
 */
export async function editAsSomeoneElse(url, token, ipn, values, reason, { mpn } = {}) {
  const headers = { Authorization: token, 'Content-Type': 'application/json' };
  const get = (path) => fetch(`${url}${path}`, { headers }).then((r) => r.json());
  const parts = await get(`/api/collections/catalog_parts/records?filter=${encodeURIComponent(`ipn = "${ipn}"`)}`);
  const part = parts.items[0];
  const savedDetails = await get(`/api/collections/catalog_part_details/records/${part.current_details}`);
  const revision = await get(`/api/collections/catalog_part_revisions/records/${part.current_revision}`);
  const details = JSON.parse(savedDetails.snapshot);
  const cad = JSON.parse(revision.snapshot);
  const changed = (fields) => fields.some((field) => field.name in values);
  const apply = (fields) =>
    fields.map((field) => (field.name in values ? { name: field.name, value: values[field.name] } : field));
  const body = {
    correlationId: `journey-${Date.now()}-${Math.floor(Math.random() * 1e6)}`,
    reason,
    ipn,
  };
  if (changed(details.fields) || mpn !== undefined) {
    body.details = {
      expectedDetails: savedDetails.number,
      fields: apply(details.fields),
      manufacturerParts:
        mpn === undefined
          ? details.manufacturer_parts
          : details.manufacturer_parts.map((part, at) => (at === 0 ? { ...part, mpn } : part)),
    };
  }
  if (changed(cad.cad_fields)) {
    body.cad = { expectedRevision: revision.number, fields: apply(cad.cad_fields) };
  }
  const reply = await fetch(`${url}/api/catalog/edits`, { method: 'POST', headers, body: JSON.stringify(body) });
  const answer = await reply.json();
  assert.equal(reply.status, 200, `someone else's edit: ${JSON.stringify(answer)}`);
  record({ event: 'edit-by-someone-else', ipn, seq: answer.seq, values, mpn });
  return answer.seq;
}

/**
 * Archives or unarchives revision `revision` of `ipn` as someone else, the
 * way another desktop would. Returns the Change's number.
 */
export async function archiveAsSomeoneElse(url, token, ipn, revision, archived, reason) {
  const reply = await fetch(`${url}/api/catalog/archives`, {
    method: 'POST',
    headers: { Authorization: token, 'Content-Type': 'application/json' },
    body: JSON.stringify({
      correlationId: `journey-${Date.now()}-${Math.floor(Math.random() * 1e6)}`,
      reason,
      ipn,
      revision,
      archived,
    }),
  });
  const answer = await reply.json();
  assert.equal(reply.status, 200, `someone else's archive: ${JSON.stringify(answer)}`);
  record({ event: 'archive-by-someone-else', ipn, revision, archived, seq: answer.seq });
  return answer.seq;
}

const LIBRARIES = 'dialog.libraries-dialog';

/** Waits until the Libraries browser is open (or closed, with `open: false`). */
export async function waitForLibraries(browser, { open = true, timeout = 10_000 } = {}) {
  await browser.waitUntil(
    async () => (await browser.execute((s) => document.querySelector(s)?.open === true, LIBRARIES)) === open,
    { timeout, timeoutMsg: open ? 'the Libraries browser did not open' : 'the Libraries browser did not close' },
  );
}

/** Waits until the browser's list shows the answer to what was last asked. */
async function librariesSettled(browser) {
  await browser.waitUntil(
    async () => browser.execute(() => document.getElementById('library-items')?.getAttribute('aria-busy') === 'false'),
    { timeout: 20_000, interval: 25, timeoutMsg: 'the list never showed its answer' },
  );
}

/**
 * In the open Libraries browser (kicad-parts-bc6f), chooses the item `name`
 * of the library `library` in `group` (`ours`, `kicad` or `installed`):
 * picks the library, searches for the name, selects its row, and chooses
 * it with Choose, or with Enter in the list when `enter` is set. Waits until
 * the browser closes.
 */
export async function chooseInLibraries(browser, { group, library, name, enter = false }) {
  await waitForLibraries(browser);
  await browser.waitUntil(
    async () =>
      browser.execute(
        (g, nick) => {
          const list = document.querySelector(`ul[aria-labelledby="library-group-${g}"]`);
          const button = list && [...list.querySelectorAll('.library-button')].find((b) => b.querySelector('.library-name')?.textContent === nick);
          button?.click();
          return Boolean(button);
        },
        group,
        library,
      ),
    { timeout: 60_000, interval: 250, timeoutMsg: `no library ${library} in ${group}` },
  );
  await librariesSettled(browser);
  await (await browser.$('#library-search')).setValue(name);
  await browser.waitUntil(
    async () =>
      browser.execute((wanted) => {
        const row = [...document.querySelectorAll('#library-items [role="option"]')].find(
          (r) => r.querySelector('.library-item-name').textContent === wanted,
        );
        row?.click();
        return Boolean(row);
      }, name),
    { timeout: 120_000, interval: 250, timeoutMsg: `no row named ${name} in ${library}` },
  );
  await waitForText(browser, '#library-detail-title', `${library}:${name}`);
  // Its drawing first, so what is drawn next comes from the same cache
  // every run and the screenshots after it never change by timing.
  await browser.waitUntil(
    async () =>
      browser.execute(() => {
        const detail = document.querySelector('.libraries-detail');
        // The browser shows each drawing in a viewer (kicad-parts-34df).
        const viewers = [...(detail?.querySelectorAll('.preview .viewer') ?? [])].filter((v) => !v.closest('dialog.zoom-dialog'));
        return (viewers.length > 0 && viewers.every((v) => v.dataset.ready === 'true')) || detail?.querySelector('.problem') != null;
      }),
    { timeout: 60_000, interval: 100, timeoutMsg: `${library}:${name} was never drawn in the browser` },
  );
  if (enter) {
    await browser.execute(() => document.getElementById('library-items').focus());
    await browser.keys('Enter');
  } else {
    await (await browser.$('#library-choose')).click();
  }
  await waitForLibraries(browser, { open: false });
  record({ event: 'chosen-in-libraries', group, library, name });
}

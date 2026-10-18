// Unit tests for the 3D view's camera rules (view3d-camera.js). They use
// the app's own copy of three.js to project points and test what the
// camera sees, so they check what a person would see on screen. No GPU is
// needed. Run: npm run js-tests --prefix apps/desktop/journeys

import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import * as THREE from '../vendor/three/three.module.js';
import {
  MAX_ZOOM,
  MIN_ZOOM,
  START_ELEVATION,
  STRAIGHT,
  fittingZoom,
  halfHeight,
  modelFrame,
  outline,
  panned,
  placeCamera,
  pointerAt,
  settled,
  standardView,
  turned,
  wheelFactor,
  wrapAngle,
  zoomed,
} from './view3d-camera.js';

const DEGREE = Math.PI / 180;
/** A view 800 by 500 pixels. */
const WIDTH = 800;
const HEIGHT = 500;
const ASPECT = WIDTH / HEIGHT;
/**
 * A part on its board, 10 mm wide, 5 mm deep and 1.6 mm high, off the
 * origin, in metres as in kicad-cli's GLB. y is up.
 */
const CENTRE = [0.012, 0.002, -0.004];
const FRAME = modelFrame(CENTRE, [0.005, 0.0008, 0.0025]);
/** A long connector: 100 mm long, 5 mm high and 8 mm deep. */
const CONNECTOR = modelFrame([0, 0.0025, 0], [0.05, 0.0025, 0.004]);
/** A tall part, such as an upright capacitor: 5 by 5 mm, 20 mm high. */
const TALL = modelFrame([0.003, 0.01, 0.001], [0.0025, 0.01, 0.0025]);
/** A model with no size. */
const EMPTY = modelFrame(null, null);

/** A three.js camera placed where `view` of the model in `frame` says. */
function cameraFor(view, frame = FRAME) {
  const camera = new THREE.PerspectiveCamera(30, ASPECT, 0.1, 10);
  placeCamera(camera, view, frame);
  return camera;
}

/** Where `point` shows on screen for `view`, from -1 to 1, y up. */
function onScreen(view, point = CENTRE, frame = FRAME) {
  const projected = new THREE.Vector3(...point).project(cameraFor(view, frame));
  return { x: projected.x, y: projected.y };
}

/** The model's box in `frame`, a hair larger, so a box of no size has one point. */
function boxOf(frame) {
  return new THREE.Box3()
    .setFromCenterAndSize(new THREE.Vector3(...frame.centre), new THREE.Vector3(...frame.halfSize).multiplyScalar(2))
    .expandByScalar(frame.radius * 1e-9);
}

/** Whether the line through the middle of the view meets the model's box. */
function middleOnModel(view, frame = FRAME) {
  const ray = new THREE.Raycaster();
  ray.setFromCamera(new THREE.Vector2(0, 0), cameraFor(view, frame));
  return ray.ray.intersectsBox(boxOf(frame));
}

/** Whether any of the model's box is in the camera's view. */
function boxInView(view, frame = FRAME) {
  const camera = cameraFor(view, frame);
  const frustum = new THREE.Frustum().setFromProjectionMatrix(
    new THREE.Matrix4().multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse),
  );
  return frustum.intersectsBox(boxOf(frame));
}

function near(actual, expected, tolerance, message) {
  assert.ok(Math.abs(actual - expected) <= tolerance, `${message}: ${actual} is not ${expected}`);
}

/** Whether every number in `view` is finite. */
function valid(view) {
  return Object.values(view).every(Number.isFinite);
}

/** Numbers from 0 to 1 that are the same every run (mulberry32). */
function seeded(seed) {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

describe('the starting view', () => {
  it("shows the model's centre in the middle, looking down at 45 degrees", () => {
    const start = standardView(FRAME, ASPECT);
    assert.deepEqual(start, { azimuth: 0, elevation: 45 * DEGREE, zoom: 1, shiftX: 0, shiftY: 0 });
    const centre = onScreen(start);
    near(centre.x, 0, 1e-9, 'the centre is in the middle across');
    near(centre.y, 0, 1e-9, 'the centre is in the middle up and down');
  });

  it('zooms out to fit a narrow view, but never past the limit', () => {
    assert.equal(fittingZoom(2), 1);
    assert.equal(fittingZoom(0.5), 0.5);
    assert.equal(fittingZoom(0.01), MIN_ZOOM);
    assert.equal(fittingZoom(Number.NaN), 1);
    assert.equal(fittingZoom(0), 1);
  });

  it('keeps the camera outside the model, and the model between near and far', () => {
    for (const frame of [FRAME, CONNECTOR, TALL, EMPTY]) {
      assert.ok(frame.distance > frame.radius * 4, `distance ${frame.distance}`);
      assert.ok(frame.near > 0 && frame.near < frame.distance - frame.radius);
      assert.ok(frame.far > frame.distance + frame.radius);
    }
  });
});

describe('turning', () => {
  it("keeps the model's centre in the same place on screen while the middle of the view stays on the model", () => {
    // Zoomed in near the middle and panned a little: the shift is less
    // than the box's smallest half-size, so it is inside every outline.
    let view = standardView(FRAME, ASPECT);
    for (let i = 0; i < 6; i += 1) view = zoomed(view, FRAME, 1.5, pointerAt(420, 245, WIDTH, HEIGHT), ASPECT);
    view = panned(view, FRAME, -30, 20, HEIGHT);
    assert.ok(Math.hypot(view.shiftX, view.shiftY) < 0.0008, `shift ${view.shiftX}, ${view.shiftY}`);
    const before = onScreen(view);
    assert.ok(Math.hypot(before.x, before.y) > 0.1, `zooming and panning moved the centre: ${before.x}, ${before.y}`);
    for (const [right, up] of [
      [30, 0],
      [0, 20],
      [-170, -60],
      [400, 10],
    ]) {
      const after = turned(view, FRAME, right * DEGREE, up * DEGREE);
      const moved = onScreen(after);
      near(moved.x, before.x, 1e-9, `after turning ${right}, ${up} the centre stays across`);
      near(moved.y, before.y, 1e-9, `after turning ${right}, ${up} the centre stays up and down`);
      // The camera turns around the centre: its distance never changes.
      const distance = (v) => cameraFor(v).position.distanceTo(new THREE.Vector3(...CENTRE));
      near(distance(after), distance(view), 1e-12, 'the camera stays as far from the centre');
    }
  });

  it('slides the view just enough, and smoothly, when a turn would leave a long connector', () => {
    // The review's case: zoomed with 12 wheel notches at one end of a
    // 100 mm connector, then turned.
    const start = standardView(CONNECTOR, ASPECT);
    const end = onScreen(start, [0.045, 0.0025, 0], CONNECTOR);
    const pointer = { x: end.x, y: end.y };
    let view = start;
    for (let i = 0; i < 12; i += 1) view = zoomed(view, CONNECTOR, wheelFactor(-100, 0), pointer, ASPECT);
    near(view.zoom, Math.exp(2.4), 1e-9, '12 notches zoom about 11 times');
    for (const degrees of [20, 60, 90, 135]) {
      const after = turned(view, CONNECTOR, degrees * DEGREE, 0);
      assert.ok(middleOnModel(after, CONNECTOR), `after turning ${degrees} degrees the middle of the view shows the connector`);
      assert.ok(boxInView(after, CONNECTOR), `after turning ${degrees} degrees the connector is in view`);
    }
    // Turned a degree at a time, the view never jumps.
    let previous = view;
    const most = [];
    for (let degrees = 1; degrees <= 360; degrees += 1) {
      const next = turned(previous, CONNECTOR, DEGREE, 0.3 * DEGREE);
      most.push(Math.hypot(next.shiftX - previous.shiftX, next.shiftY - previous.shiftY));
      previous = next;
    }
    const step = Math.max(...most);
    assert.ok(step < CONNECTOR.radius * 0.03, `one degree moved the view by ${step}, the radius is ${CONNECTOR.radius}`);
  });

  it('stops just short of straight down and up, and wraps the azimuth', () => {
    let view = standardView(FRAME, ASPECT);
    view = turned(view, FRAME, 0, 200 * DEGREE);
    assert.equal(view.elevation, STRAIGHT);
    view = turned(view, FRAME, 0, -500 * DEGREE);
    assert.equal(view.elevation, -STRAIGHT);
    for (let i = 0; i < 1000; i += 1) view = turned(view, FRAME, 37 * DEGREE, 0);
    assert.ok(view.azimuth > -Math.PI && view.azimuth <= Math.PI, `azimuth ${view.azimuth}`);
    assert.equal(wrapAngle(Math.PI), Math.PI, 'Bottom stays at 180 degrees');
    assert.equal(wrapAngle(-Math.PI), Math.PI);
    near(wrapAngle(-30 * DEGREE), -30 * DEGREE, 1e-15, 'a small angle stays');
  });
});

describe('zooming', () => {
  it('keeps the point under the pointer still', () => {
    const start = standardView(FRAME, ASPECT);
    const pointer = pointerAt(480, 200, WIDTH, HEIGHT);
    // The model's point that shows under the pointer, in the plane
    // through its centre.
    const camera = cameraFor(start);
    const ray = new THREE.Raycaster();
    ray.setFromCamera(new THREE.Vector2(pointer.x, pointer.y), camera);
    const facing = camera.getWorldDirection(new THREE.Vector3());
    const plane = new THREE.Plane().setFromNormalAndCoplanarPoint(facing, new THREE.Vector3(...CENTRE));
    const under = ray.ray.intersectPlane(plane, new THREE.Vector3()).toArray();
    const view = zoomed(start, FRAME, 1.4, pointer, ASPECT);
    assert.equal(view.zoom, 1.4);
    const after = onScreen(view, under);
    near(after.x, pointer.x, 1e-9, 'the point stays under the pointer across');
    near(after.y, pointer.y, 1e-9, 'the point stays under the pointer up and down');
  });

  it('zooms at the middle with no pointer, as plus and minus do', () => {
    const start = standardView(FRAME, ASPECT);
    const view = zoomed(start, FRAME, 1.5, null, ASPECT);
    assert.deepEqual({ ...view, zoom: undefined }, { ...start, zoom: undefined });
    near(zoomed(view, FRAME, 1 / 1.5, null, ASPECT).zoom, 1, 1e-12, 'minus undoes plus');
  });

  it('stops at its limits, however many wheel turns come', () => {
    let view = standardView(FRAME, ASPECT);
    for (let i = 0; i < 500; i += 1) view = zoomed(view, FRAME, wheelFactor(-120, 0), pointerAt(10, 10, WIDTH, HEIGHT), ASPECT);
    assert.equal(view.zoom, MAX_ZOOM);
    assert.ok(valid(view) && middleOnModel(view), 'zoomed in at an empty corner, the middle still shows the model');
    for (let i = 0; i < 500; i += 1) view = zoomed(view, FRAME, wheelFactor(120, 0), pointerAt(790, 490, WIDTH, HEIGHT), ASPECT);
    assert.equal(view.zoom, MIN_ZOOM);
    assert.ok(valid(view));
    // Zoomed right out, the model still shows, about a quarter of the
    // view's height or more.
    const shown = (2 * FRAME.radius) / (2 * halfHeight(view, FRAME));
    assert.ok(shown > 0.2, `the model fills ${shown} of the view's height`);
  });

  it('limits each wheel event, and ignores broken ones', () => {
    assert.equal(wheelFactor(0, 0), 1);
    near(wheelFactor(-100, 0), Math.exp(0.2), 1e-12, 'one notch of 100 pixels');
    near(wheelFactor(-3, 1), Math.exp(0.24), 1e-12, 'lines are 40 pixels');
    // A trackpad fling or a high-resolution wheel.
    assert.equal(wheelFactor(-1e9, 0), Math.exp(0.5));
    assert.equal(wheelFactor(1e9, 2), Math.exp(-0.5));
    for (const broken of [Number.NaN, Infinity, -Infinity, undefined, null, '5']) {
      assert.equal(wheelFactor(broken, 0), 1, String(broken));
    }
  });
});

describe('panning', () => {
  it('moves the model with the pointer', () => {
    const start = standardView(FRAME, ASPECT);
    const view = panned(start, FRAME, 40, -25, HEIGHT);
    const centre = onScreen(view);
    // 40 pixels right and 25 up, as -1 to 1 across the view.
    near(centre.x, (40 / WIDTH) * 2, 1e-9, 'it moved right');
    near(centre.y, (25 / HEIGHT) * 2, 1e-9, 'it moved up');
  });

  it("stops with the middle of the view at the edge of the model's outline", () => {
    let view = standardView(FRAME, ASPECT);
    for (let i = 0; i < 1000; i += 1) view = panned(view, FRAME, 300, -200, HEIGHT);
    assert.ok(middleOnModel(view), 'the middle of the view still shows the model');
    // From the front at 45 degrees the outline is a rectangle: the box's
    // width across, and its depth and height, tilted, up and down.
    const round = (value) => Math.round(value * 1e12) / 1e12;
    const top = round(Math.SQRT1_2 * (0.0008 + 0.0025));
    assert.deepEqual(
      outline(view, FRAME).map((corner) => corner.map(round)),
      [
        [-0.005, -top],
        [0.005, -top],
        [0.005, top],
        [-0.005, top],
      ],
    );
    near(view.shiftX, -0.005, 1e-15, 'a pan stops at the left edge');
    near(view.shiftY, -Math.SQRT1_2 * (0.0008 + 0.0025), 1e-15, 'a pan stops at the bottom edge');
    // Zoomed out, the model's centre is on screen.
    view = zoomed(view, FRAME, 1e-9, null, ASPECT);
    const centre = onScreen(view);
    assert.ok(Math.abs(centre.x) < 1 && Math.abs(centre.y) < 1, `the centre is at ${centre.x}, ${centre.y}`);
  });
});

describe('any sequence of turns, pans and zooms', () => {
  // 3,000 random steps on each model, some with broken numbers. After
  // each one the numbers are valid, every limit holds, the middle of the
  // view shows the model, and the camera is outside it.
  const models = { 'a part on its board': FRAME, 'a long connector': CONNECTOR, 'a tall part': TALL, 'an empty model': EMPTY };
  for (const [name, frame] of Object.entries(models)) {
    it(`keeps ${name} in view`, () => {
      const random = seeded(4588);
      const box = boxOf(frame);
      const broken = [Number.NaN, Infinity, -Infinity, 1e300, -1e300];
      const some = (scale) => (random() < 0.02 ? broken[Math.floor(random() * broken.length)] : (random() * 2 - 1) * scale);
      let view = standardView(frame, ASPECT);
      for (let step = 0; step < 3000; step += 1) {
        const kind = random();
        if (kind < 0.35) view = turned(view, frame, some(Math.PI / 2), some(Math.PI / 2));
        else if (kind < 0.65) view = panned(view, frame, some(600), some(600), HEIGHT);
        else if (kind < 0.98) view = zoomed(view, frame, wheelFactor(some(800), 0), pointerAt(random() * WIDTH, random() * HEIGHT, WIDTH, HEIGHT), ASPECT);
        else view = standardView(frame, ASPECT);
        const where = `step ${step}: ${JSON.stringify(view)}`;
        assert.ok(valid(view), where);
        assert.ok(view.zoom >= MIN_ZOOM && view.zoom <= MAX_ZOOM, where);
        assert.ok(Math.abs(view.elevation) <= STRAIGHT && Math.abs(view.azimuth) <= Math.PI, where);
        assert.ok(middleOnModel(view, frame), `the middle of the view left the model at ${where}`);
        assert.ok(boxInView(view, frame), `the model left the view at ${where}`);
        assert.ok(!box.containsPoint(cameraFor(view, frame).position), `the camera went inside the model at ${where}`);
      }
    });
  }
});

describe('broken input', () => {
  /** A view zoomed in near a corner of the model, then panned. */
  function zoomedNearACorner() {
    let view = standardView(FRAME, ASPECT);
    for (let i = 0; i < 6; i += 1) view = zoomed(view, FRAME, 1.5, pointerAt(600, 150, WIDTH, HEIGHT), ASPECT);
    return panned(view, FRAME, -30, 20, HEIGHT);
  }

  it('changes nothing for numbers that are not finite, or a drag of no size', () => {
    const view = zoomedNearACorner();
    for (const broken of [Number.NaN, Infinity, -Infinity, undefined]) {
      assert.deepEqual(turned(view, FRAME, broken, 0), view);
      assert.deepEqual(panned(view, FRAME, 0, broken, HEIGHT), view);
      assert.deepEqual(zoomed(view, FRAME, broken, null, ASPECT), view);
    }
    assert.deepEqual(zoomed(view, FRAME, 0, null, ASPECT), view);
    assert.deepEqual(zoomed(view, FRAME, -2, null, ASPECT), view);
    assert.deepEqual(turned(view, FRAME, 0, 0), view);
    assert.deepEqual(panned(view, FRAME, 0, 0, HEIGHT), view);
  });

  it('copes with a view of no size and a pointer it cannot place', () => {
    const view = standardView(FRAME, ASPECT);
    assert.equal(pointerAt(10, 10, 0, 0), null);
    assert.equal(pointerAt(Number.NaN, 10, WIDTH, HEIGHT), null);
    assert.ok(valid(panned(view, FRAME, 10, 10, 0)));
    assert.ok(valid(panned(view, FRAME, 10, 10, Number.NaN)));
    assert.ok(valid(zoomed(view, FRAME, 2, { x: Infinity, y: 0 }, 0)));
    assert.ok(valid(standardView(FRAME, Number.NaN)));
  });

  it('puts broken camera numbers back to the starting view, and limits the rest', () => {
    const fixed = settled({ azimuth: Number.NaN, elevation: Infinity, zoom: -1, shiftX: undefined, shiftY: 3 }, FRAME, ASPECT);
    // Numbers that are not finite go back; finite ones are only limited.
    assert.equal(fixed.azimuth, 0);
    assert.equal(fixed.elevation, START_ELEVATION);
    assert.equal(fixed.zoom, MIN_ZOOM);
    assert.equal(fixed.shiftX, 0);
    near(fixed.shiftY, Math.SQRT1_2 * (0.0008 + 0.0025), 1e-15, 'a shift of 3 m stops at the top edge of the model');
    assert.equal(settled({ ...fixed, elevation: 4 }, FRAME, ASPECT).elevation, STRAIGHT);
    assert.deepEqual(settled(null, FRAME, ASPECT), standardView(FRAME, ASPECT));
  });

  it('draws an empty or zero-size model as a small one at the origin', () => {
    for (const [centre, halfSize] of [
      [[0, 0, 0], [0, 0, 0]],
      [[Number.NaN, 0, 0], [Number.NaN, 0, 0]],
      [undefined, [Infinity, 1, 1]],
      [[1, 2], -1],
      [null, undefined],
    ]) {
      const frame = modelFrame(centre, halfSize);
      assert.ok([...frame.centre, frame.radius, frame.distance, frame.near, frame.far].every(Number.isFinite), JSON.stringify(frame));
      assert.ok(frame.near > 0 && frame.far > frame.near);
    }
    assert.deepEqual(EMPTY.centre, [0, 0, 0]);
    // A model of one point is tiny, not nothing, and it can't be panned
    // away from.
    const point = modelFrame([1, 1, 1], [1e-12, 0, 0]);
    assert.ok(point.radius >= 1e-5);
    const view = panned(standardView(point, ASPECT), point, 500, 500, HEIGHT);
    assert.ok(Math.abs(view.shiftX) <= 1e-12 && view.shiftY === 0);
  });
});

describe('Reset view', () => {
  it('brings back the starting view exactly, from anywhere', () => {
    const start = standardView(FRAME, ASPECT);
    let view = zoomed(start, FRAME, 7, pointerAt(600, 150, WIDTH, HEIGHT), ASPECT);
    view = panned(view, FRAME, -30, 20, HEIGHT);
    view = turned(view, FRAME, 123 * DEGREE, -77 * DEGREE);
    assert.notDeepEqual(view, start);
    // Reset view makes the starting view again from nothing but the model
    // and the aspect.
    assert.deepEqual(standardView(FRAME, ASPECT), start);
    const camera = cameraFor(view);
    placeCamera(camera, standardView(FRAME, ASPECT), FRAME);
    const fresh = cameraFor(start);
    assert.deepEqual(camera.projectionMatrix.elements, fresh.projectionMatrix.elements);
    assert.deepEqual(camera.matrixWorld.elements, fresh.matrixWorld.elements);
  });
});

// Unit tests for the 3D view's camera rules (view3d-camera.js). They use
// the app's own copy of three.js to project points, so they check what a
// person would see on screen. Run: node --test "apps/desktop-ui/js/*.test.mjs"

import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import * as THREE from '../vendor/three/three.module.js';
import {
  MAX_ZOOM,
  MIN_ZOOM,
  START_ELEVATION,
  STRAIGHT,
  boxReach,
  fittingZoom,
  halfHeight,
  modelFrame,
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

/** Where `point` shows on screen for `view`, from -1 to 1, y up. */
function onScreen(view, point = CENTRE) {
  const camera = new THREE.PerspectiveCamera(30, ASPECT, 0.1, 10);
  placeCamera(camera, view, FRAME);
  const projected = new THREE.Vector3(...point).project(camera);
  return { x: projected.x, y: projected.y };
}

/** How far the camera is from `point`. */
function cameraDistance(view, point = CENTRE) {
  const camera = new THREE.PerspectiveCamera(30, ASPECT, 0.1, 10);
  placeCamera(camera, view, FRAME);
  return camera.position.distanceTo(new THREE.Vector3(...point));
}

function near(actual, expected, tolerance, message) {
  assert.ok(Math.abs(actual - expected) <= tolerance, `${message}: ${actual} is not ${expected}`);
}

/** Whether every number in `view` is finite. */
function valid(view) {
  return Object.values(view).every(Number.isFinite);
}

/** A view zoomed in at a corner of the screen, then panned. */
function zoomedIntoACorner() {
  let view = standardView(ASPECT);
  for (let i = 0; i < 6; i += 1) view = zoomed(view, FRAME, 1.5, pointerAt(700, 80, WIDTH, HEIGHT), ASPECT);
  return panned(view, FRAME, -30, 20, HEIGHT);
}

describe('the starting view', () => {
  it("shows the model's centre in the middle, looking down at 45 degrees", () => {
    const start = standardView(ASPECT);
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
    assert.ok(FRAME.distance > FRAME.radius * 4, `distance ${FRAME.distance}`);
    assert.ok(FRAME.near > 0 && FRAME.near < FRAME.distance - FRAME.radius);
    assert.ok(FRAME.far > FRAME.distance + FRAME.radius);
  });
});

describe('turning', () => {
  it("keeps the model's centre in the same place on screen after zooming at a corner and panning", () => {
    const view = zoomedIntoACorner();
    const before = onScreen(view);
    // Off the middle: zooming at the corner and panning moved the view.
    assert.ok(Math.hypot(before.x, before.y) > 0.1, `the centre is at ${before.x}, ${before.y}`);
    for (const [right, up] of [
      [30, 0],
      [0, 20],
      [-170, -60],
      [400, 10],
    ]) {
      const after = turned(view, right * DEGREE, up * DEGREE);
      const moved = onScreen(after);
      near(moved.x, before.x, 1e-9, `after turning ${right}, ${up} the centre stays across`);
      near(moved.y, before.y, 1e-9, `after turning ${right}, ${up} the centre stays up and down`);
      // The camera turns around the centre: its distance never changes.
      near(cameraDistance(after), cameraDistance(view), 1e-12, 'the camera stays as far from the centre');
    }
  });

  it('stops just short of straight down and up, and wraps the azimuth', () => {
    let view = standardView(ASPECT);
    view = turned(view, 0, 200 * DEGREE);
    assert.equal(view.elevation, STRAIGHT);
    view = turned(view, 0, -500 * DEGREE);
    assert.equal(view.elevation, -STRAIGHT);
    for (let i = 0; i < 1000; i += 1) view = turned(view, 37 * DEGREE, 0);
    assert.ok(view.azimuth > -Math.PI && view.azimuth <= Math.PI, `azimuth ${view.azimuth}`);
    assert.equal(wrapAngle(Math.PI), Math.PI, 'Bottom stays at 180 degrees');
    assert.equal(wrapAngle(-Math.PI), Math.PI);
    near(wrapAngle(-30 * DEGREE), -30 * DEGREE, 1e-15, 'a small angle stays');
  });
});

describe('zooming', () => {
  it('keeps the point under the pointer still', () => {
    const start = standardView(ASPECT);
    const pointer = pointerAt(620, 140, WIDTH, HEIGHT);
    // The model's point that shows under the pointer, in the plane
    // through its centre.
    const camera = new THREE.PerspectiveCamera(30, ASPECT, 0.1, 10);
    placeCamera(camera, start, FRAME);
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
    const start = standardView(ASPECT);
    const view = zoomed(start, FRAME, 1.5, null, ASPECT);
    assert.deepEqual({ ...view, zoom: undefined }, { ...start, zoom: undefined });
    near(zoomed(view, FRAME, 1 / 1.5, null, ASPECT).zoom, 1, 1e-12, 'minus undoes plus');
  });

  it('stops at its limits, however many wheel turns come', () => {
    let view = standardView(ASPECT);
    for (let i = 0; i < 500; i += 1) view = zoomed(view, FRAME, wheelFactor(-120, 0), pointerAt(10, 10, WIDTH, HEIGHT), ASPECT);
    assert.equal(view.zoom, MAX_ZOOM);
    assert.ok(valid(view));
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
    const start = standardView(ASPECT);
    const view = panned(start, FRAME, 40, -25, HEIGHT);
    const centre = onScreen(view);
    // 40 pixels right and 25 up, as -1 to 1 across the view.
    near(centre.x, (40 / WIDTH) * 2, 1e-9, 'it moved right');
    near(centre.y, (25 / HEIGHT) * 2, 1e-9, 'it moved up');
  });

  it("keeps the middle of the view over the model's box", () => {
    let view = standardView(ASPECT);
    for (let i = 0; i < 1000; i += 1) view = panned(view, FRAME, 300, -200, HEIGHT);
    const reach = boxReach(view, FRAME);
    assert.deepEqual([view.shiftX, view.shiftY], [-reach.x, -reach.y], 'a pan stops at the edge of the box');
    // From the front at 45 degrees: the box's width across, and its depth
    // and height, tilted, up and down.
    near(reach.x, 0.005, 1e-15, 'the box reaches across by half its width');
    near(reach.y, Math.SQRT1_2 * (0.0008 + 0.0025), 1e-15, 'the box reaches up by its tilted depth and height');
    // Zooming in at the far corner, over empty space, stays at the edge.
    for (let i = 0; i < 100; i += 1) view = zoomed(view, FRAME, 1.5, pointerAt(0, 0, WIDTH, HEIGHT), ASPECT);
    assert.ok(Math.abs(view.shiftX) <= reach.x && Math.abs(view.shiftY) <= reach.y);
    // The box reaches no further than the model's radius, whichever way
    // the camera looks, so the shift never does either.
    for (let azimuth = -180; azimuth <= 180; azimuth += 15) {
      for (let elevation = -89; elevation <= 89; elevation += 17) {
        const turnedView = { ...view, azimuth: azimuth * DEGREE, elevation: elevation * DEGREE };
        const { x, y } = boxReach(turnedView, FRAME);
        assert.ok(x <= FRAME.radius && y <= FRAME.radius, `${azimuth}, ${elevation}`);
      }
    }
    // Zoomed out, the model's centre is on screen.
    view = zoomed(view, FRAME, 1e-9, null, ASPECT);
    const centre = onScreen(view);
    assert.ok(Math.abs(centre.x) < 1 && Math.abs(centre.y) < 1, `the centre is at ${centre.x}, ${centre.y}`);
  });

  it('never jumps after a turn has carried the pan past the edge of the box', () => {
    // Pan to the right edge from the front, then turn to look along the
    // board's short side, which reaches less far across.
    let view = standardView(ASPECT);
    for (let i = 0; i < 100; i += 1) view = panned(view, FRAME, -300, 0, HEIGHT);
    view = turned(view, 90 * DEGREE, 0);
    assert.ok(view.shiftX > boxReach(view, FRAME).x, 'the pan is now past the edge');
    // A small pan moves the view by just that much, either way.
    const step = (2 * halfHeight(view, FRAME)) / HEIGHT;
    near(panned(view, FRAME, 1, 0, HEIGHT).shiftX, view.shiftX - step, 1e-15, 'a pan back moves one pixel');
    assert.equal(panned(view, FRAME, -1, 0, HEIGHT).shiftX, view.shiftX, 'a pan further out stops');
    assert.equal(zoomed(view, FRAME, 1.2, null, ASPECT).shiftX, view.shiftX, 'a zoom at the middle keeps it');
  });
});

describe('broken input', () => {
  it('changes nothing for numbers that are not finite, or a drag of no size', () => {
    const view = zoomedIntoACorner();
    for (const broken of [Number.NaN, Infinity, -Infinity, undefined]) {
      assert.deepEqual(turned(view, broken, 0), view);
      assert.deepEqual(panned(view, FRAME, 0, broken, HEIGHT), view);
      assert.deepEqual(zoomed(view, FRAME, broken, null, ASPECT), view);
    }
    assert.deepEqual(zoomed(view, FRAME, 0, null, ASPECT), view);
    assert.deepEqual(zoomed(view, FRAME, -2, null, ASPECT), view);
    assert.deepEqual(turned(view, 0, 0), view);
    assert.deepEqual(panned(view, FRAME, 0, 0, HEIGHT), view);
  });

  it('copes with a view of no size and a pointer it cannot place', () => {
    const view = standardView(ASPECT);
    assert.equal(pointerAt(10, 10, 0, 0), null);
    assert.equal(pointerAt(Number.NaN, 10, WIDTH, HEIGHT), null);
    assert.ok(valid(panned(view, FRAME, 10, 10, 0)));
    assert.ok(valid(panned(view, FRAME, 10, 10, Number.NaN)));
    assert.ok(valid(zoomed(view, FRAME, 2, { x: Infinity, y: 0 }, 0)));
    assert.ok(valid(standardView(Number.NaN)));
  });

  it('puts broken camera numbers back to the starting view', () => {
    // Numbers that are not finite go back; finite ones are only limited.
    const fixed = settled({ azimuth: Number.NaN, elevation: Infinity, zoom: -1, shiftX: undefined, shiftY: 3 }, ASPECT);
    assert.deepEqual(fixed, { azimuth: 0, elevation: START_ELEVATION, zoom: MIN_ZOOM, shiftX: 0, shiftY: 3 });
    assert.equal(settled({ ...fixed, elevation: 4 }, ASPECT).elevation, STRAIGHT);
    assert.deepEqual(settled(null, ASPECT), standardView(ASPECT));
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
    assert.deepEqual(modelFrame(undefined, null).centre, [0, 0, 0]);
    // A model of one point is tiny, not nothing, and it can't be panned
    // away from.
    const point = modelFrame([1, 1, 1], [1e-12, 0, 0]);
    assert.ok(point.radius >= 1e-5);
    const view = panned(standardView(ASPECT), point, 500, 500, HEIGHT);
    assert.ok(Math.abs(view.shiftX) <= 1e-12 && view.shiftY === 0);
  });
});

describe('Reset view', () => {
  it('brings back the starting view exactly, from anywhere', () => {
    const start = standardView(ASPECT);
    let view = zoomedIntoACorner();
    view = turned(view, 123 * DEGREE, -77 * DEGREE);
    assert.notDeepEqual(view, start);
    // Reset view makes the starting view again from nothing but the aspect.
    assert.deepEqual(standardView(ASPECT), start);
    const camera = new THREE.PerspectiveCamera(30, ASPECT, 0.1, 10);
    placeCamera(camera, view, FRAME);
    placeCamera(camera, standardView(ASPECT), FRAME);
    const fresh = new THREE.PerspectiveCamera(30, ASPECT, 0.1, 10);
    placeCamera(fresh, start, FRAME);
    assert.deepEqual(camera.projectionMatrix.elements, fresh.projectionMatrix.elements);
    assert.deepEqual(camera.matrixWorld.elements, fresh.matrixWorld.elements);
  });
});

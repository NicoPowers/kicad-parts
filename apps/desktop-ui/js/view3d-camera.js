// The 3D view's camera rules (kicad-parts-4588). This file holds only
// maths on plain numbers, so `node --test` checks it without a browser
// (view3d-camera.test.mjs). view3d.js applies it to the three.js camera.
//
// How the camera moves
// --------------------
// The model always turns around its own centre, so the whole part turns
// steadily, even after zooming into a corner.
//
// - The camera stays at one distance from the centre, worked out from the
//   model's size so that it is always outside the model.
// - Turning changes two angles: the azimuth, around the vertical, and the
//   elevation, up and down.
// - Zooming narrows or widens the camera's view, as a zoom lens does
//   (three.js's camera.zoom). The camera never moves nearer, so it can
//   never go inside the model, and nothing is ever cut off.
// - Panning, and zooming at the pointer, slide the camera sideways in its
//   own plane. We call this the shift. It is held in the camera's own
//   right and up directions, so turning carries it along. The centre then
//   stays at the same place on screen while the model turns.
//
// Why not move the turning point, as three.js's OrbitControls does? Then
// a pan or a zoom at the pointer moves the point the model turns around,
// and later turns swing the model away. That is what people found hard.
//
// Limits, so the view can never be lost or broken
// ------------------------------------------------
// - Zoom stays between MIN_ZOOM and MAX_ZOOM of the size that fits the
//   model's height.
// - Panning and zooming keep the middle of the view over the model's box,
//   as the camera sees it. A pan stops at the box's edge, and a zoom over
//   empty space pulls towards the model. Turning keeps the shift, so the
//   box's edge may then lie nearer than the shift. A pan or zoom never
//   pushes the view further out then, and never jumps it back either.
//   The shift stays within the model's radius across and up, always.
// - The elevation stops just short of straight down or up, and the
//   azimuth wraps around to stay between -180 and 180 degrees.
// - Every input is checked: a number that is not finite is ignored.
//   settled() puts the angles and zoom back in bounds, and a value that
//   is not finite back to the starting view's.
// So the numbers stay valid after any number of wheel turns or drags.
//
// Units: angles in radians, lengths in the model's units (kicad-cli's
// GLB is in metres), and pointer positions from -1 to 1 across the view.

const DEGREE = Math.PI / 180;

/** The camera's field of view from top to bottom, in degrees. */
export const FIELD_OF_VIEW = 30;
/** The starting view looks down at the front at this angle. */
export const START_ELEVATION = 45 * DEGREE;
/** Top and Bottom stop just short of straight down or up. */
export const STRAIGHT = 89.9 * DEGREE;
/** Room around the model at zoom 1: its sphere fills 1 / 1.15 of the view's height. */
const FIT_MARGIN = 1.15;
/** Zoomed out, the model fills about a quarter of the view's height. */
export const MIN_ZOOM = 0.25;
/** Zoomed in, the view shows a 50th of the height it shows at zoom 1. */
export const MAX_ZOOM = 50;
/** One wheel event zooms by at most e^0.5, about 1.65 times. */
const MAX_WHEEL_STEP = 0.5;
/** How much one wheel pixel zooms. */
const WHEEL_PER_PIXEL = 0.002;
/** A model smaller than this, such as an empty one, is treated as this size. */
const MIN_RADIUS = 1e-5;
/** The size used when the model has no size at all: 1 mm. */
const EMPTY_RADIUS = 1e-3;

const HALF_FIELD = (FIELD_OF_VIEW / 2) * DEGREE;

/** Whether every argument is a finite number. */
function finite(...values) {
  return values.every((value) => typeof value === 'number' && Number.isFinite(value));
}

function clamp(value, low, high) {
  return Math.min(high, Math.max(low, value));
}

/** `angle` moved by whole turns to lie above -180 and up to 180 degrees. */
export function wrapAngle(angle) {
  return angle - 2 * Math.PI * Math.ceil((angle - Math.PI) / (2 * Math.PI));
}

/** Whether `value` is a list of three finite numbers. */
function isPoint(value) {
  return Array.isArray(value) && value.length === 3 && finite(...value);
}

/**
 * The model the camera looks at, from its box: the box's `centre` and
 * `halfSize`, each [x, y, z]. Also the model's radius (half the box's
 * diagonal), the camera's fixed distance from the centre, and the near and
 * far limits of what it draws. An empty or broken box gives a small model
 * at the origin.
 */
export function modelFrame(centre, halfSize) {
  const point = isPoint(centre) ? [...centre] : [0, 0, 0];
  const half = isPoint(halfSize) ? halfSize.map(Math.abs) : [0, 0, 0];
  const diagonal = Math.hypot(...half);
  const size = diagonal > 0 ? Math.max(diagonal, MIN_RADIUS) : EMPTY_RADIUS;
  // At this distance the whole model fits the view's height at zoom 1,
  // and the camera is well outside the model's sphere.
  const distance = (size * FIT_MARGIN) / Math.sin(HALF_FIELD);
  return {
    centre: point,
    halfSize: half,
    radius: size,
    distance,
    // Every point of the model lies between distance - radius and
    // distance + radius in front of the camera, whatever the shift.
    near: (distance - size) / 2,
    far: (distance + size) * 2,
  };
}

/** The zoom at which the model fits a view `aspect` wide for each unit high. */
export function fittingZoom(aspect) {
  const wide = finite(aspect) && aspect > 0 ? aspect : 1;
  // A narrow view must fit the model's width: zoom out by its aspect.
  return clamp(Math.min(1, wide), MIN_ZOOM, 1);
}

/**
 * A fitted view from `azimuth` and `elevation`, centred on the model.
 * With no angles it is the starting view: down at the front, 45 degrees up.
 */
export function standardView(aspect, azimuth = 0, elevation = START_ELEVATION) {
  return settled({ azimuth, elevation, zoom: fittingZoom(aspect), shiftX: 0, shiftY: 0 }, aspect);
}

/**
 * `view` with every value valid and inside its limits. A value that is
 * not finite goes back to the starting view's.
 */
export function settled(view, aspect) {
  const start = { azimuth: 0, elevation: START_ELEVATION, zoom: fittingZoom(aspect), shiftX: 0, shiftY: 0 };
  const pick = (name) => (finite(view?.[name]) ? view[name] : start[name]);
  const azimuth = wrapAngle(pick('azimuth'));
  const elevation = clamp(pick('elevation'), -STRAIGHT, STRAIGHT);
  const zoom = clamp(pick('zoom'), MIN_ZOOM, MAX_ZOOM);
  return { azimuth, elevation, zoom, shiftX: pick('shiftX'), shiftY: pick('shiftY') };
}

/**
 * The camera's directions for `view`, each [x, y, z]: `back`, from the
 * centre towards the camera, and the camera's own `right` and `up`, as
 * three.js's lookAt makes them with the world's y up.
 */
function axes(view) {
  const flat = Math.cos(view.elevation);
  const lift = Math.sin(view.elevation);
  const across = Math.sin(view.azimuth);
  const along = Math.cos(view.azimuth);
  return {
    back: [flat * across, lift, flat * along],
    right: [along, 0, -across],
    up: [-lift * across, flat, -lift * along],
  };
}

/**
 * How far the model's box reaches from its centre along the camera's
 * right and up, for `view`: { x, y }. It is never more than the radius.
 */
export function boxReach(view, frame) {
  const { right, up } = axes(view);
  const reach = (direction) => direction.reduce((sum, part, i) => sum + Math.abs(part) * frame.halfSize[i], 0);
  return { x: reach(right), y: reach(up) };
}

/**
 * `moved` with the middle of the view kept over the model's box, or no
 * further out than it was in `before`. So a pan or zoom never jumps the
 * view after a turn has carried the shift past the box's edge.
 */
function overModel(moved, before, frame) {
  const reach = boxReach(moved, frame);
  const limit = (value, was, most) => clamp(value, Math.min(-most, was), Math.max(most, was));
  return {
    ...moved,
    shiftX: limit(moved.shiftX, before.shiftX, reach.x),
    shiftY: limit(moved.shiftY, before.shiftY, reach.y),
  };
}

/** Half the height the view shows, in the plane through the model's centre. */
export function halfHeight(view, frame) {
  return (frame.distance * Math.tan(HALF_FIELD)) / view.zoom;
}

/** `view` turned `right` and `up` (radians), around the model's centre. */
export function turned(view, right, up) {
  if (!finite(right, up)) return view;
  return settled({ ...view, azimuth: view.azimuth - right, elevation: view.elevation + up });
}

/**
 * `view` panned as by a drag of `dx`, `dy` pixels on a view `height`
 * pixels high: the model follows the pointer.
 */
export function panned(view, frame, dx, dy, height) {
  if (!finite(dx, dy)) return view;
  const perPixel = (2 * halfHeight(view, frame)) / Math.max(finite(height) ? height : 1, 1);
  const moved = { ...view, shiftX: view.shiftX - dx * perPixel, shiftY: view.shiftY + dy * perPixel };
  return overModel(settled(moved), view, frame);
}

/**
 * `view` zoomed in by `factor` (more than 1 zooms in) at `pointer`
 * ({ x, y }, each -1 to 1, y up), on a view `aspect` wide for each unit
 * high. The point under the pointer stays under it, unless a limit stops
 * the zoom or the shift. With no pointer it zooms at the view's middle.
 */
export function zoomed(view, frame, factor, pointer, aspect) {
  if (!finite(factor) || factor <= 0) return view;
  const zoom = clamp(view.zoom * factor, MIN_ZOOM, MAX_ZOOM);
  const at = pointer && finite(pointer.x, pointer.y) ? { x: clamp(pointer.x, -1, 1), y: clamp(pointer.y, -1, 1) } : { x: 0, y: 0 };
  const wide = finite(aspect) && aspect > 0 ? aspect : 1;
  // How far the pointer is from the view's middle, in the plane through
  // the centre, before and after.
  const before = halfHeight(view, frame);
  const after = halfHeight({ ...view, zoom }, frame);
  const moved = {
    ...view,
    zoom,
    shiftX: view.shiftX + at.x * wide * (before - after),
    shiftY: view.shiftY + at.y * (before - after),
  };
  return overModel(settled(moved, aspect), view, frame);
}

/**
 * How much one wheel event zooms in: more than 1 zooms in. `deltaMode` is
 * the event's: 0 for pixels, 1 for lines and 2 for pages. A trackpad or a
 * fine wheel sends many small events; any one event is limited.
 */
export function wheelFactor(deltaY, deltaMode) {
  if (!finite(deltaY)) return 1;
  // Lines and pages are about 40 and 800 pixels.
  const unit = deltaMode === 1 ? 40 : deltaMode === 2 ? 800 : 1;
  return Math.exp(clamp(-deltaY * unit * WHEEL_PER_PIXEL, -MAX_WHEEL_STEP, MAX_WHEEL_STEP));
}

/**
 * The pointer at `x`, `y` pixels in a view `width` by `height` pixels, as
 * -1 to 1 across it, with y up. Null when it can't be told.
 */
export function pointerAt(x, y, width, height) {
  if (!finite(x, y, width, height) || width <= 0 || height <= 0) return null;
  return { x: (x / width) * 2 - 1, y: 1 - (y / height) * 2 };
}

/**
 * Where the camera is and what it looks at, for `view` of the model in
 * `frame`: { position, target } as [x, y, z], with the world's y up.
 */
export function pose(view, frame) {
  const { back, right, up } = axes(view);
  // The shift slides the camera in its own plane; it looks the same way.
  const target = frame.centre.map((value, i) => value + right[i] * view.shiftX + up[i] * view.shiftY);
  const position = target.map((value, i) => value + back[i] * frame.distance);
  return { position, target };
}

/**
 * Puts a three.js PerspectiveCamera where `view` says. The caller sets
 * its aspect. It looks along the turn direction from the shifted place,
 * so the model's centre stays put on screen while the model turns.
 */
export function placeCamera(camera, view, frame) {
  const { position, target } = pose(view, frame);
  camera.fov = FIELD_OF_VIEW;
  camera.position.set(...position);
  camera.up.set(0, 1, 0);
  camera.lookAt(...target);
  camera.zoom = view.zoom;
  camera.near = frame.near;
  camera.far = frame.far;
  camera.updateProjectionMatrix();
  // The renderer does this too; doing it here lets project() work at once.
  camera.updateMatrixWorld();
}

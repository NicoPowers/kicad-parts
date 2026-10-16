// A footprint's 3D view (kicad-parts-c9c1), drawn with three.js.
//
// three.js is copied into the app (vendor/three, see SOURCE.txt) and
// loaded from the app itself the first time a person turns 3D on, never
// from the network. The GLB comes from the app as bytes and is read with
// GLTFLoader.parse, so nothing is fetched.
//
// The app's CSP needs nothing new for this: three.js runs no eval, and
// kicad-cli's GLB holds no textures, which GLTFLoader would load from
// blob: URLs. A texture in some model would be refused, not the model.
//
// Controls, on the focused view area:
// - a left drag turns the model around its centre;
// - the wheel zooms at the pointer;
// - a middle or right drag moves (pans) the view;
// - the arrow keys turn, Shift and the arrow keys pan, + and - zoom, and
//   0 shows the starting view again.
//
// Leptos calls dispose() when the 3D view or its close-up closes. It frees
// the GPU memory and removes every listener, so opening the view again
// and again does not leak.

let loading = null;

/** three.js and its GLB reader, loaded once. */
function loadThree() {
  loading ??= Promise.all([
    import('/three/three.module.js'),
    import('/three/addons/loaders/GLTFLoader.js'),
  ]).then(([THREE, loader]) => ({ THREE, GLTFLoader: loader.GLTFLoader }));
  // A failed load may work next time.
  loading.catch(() => {
    loading = null;
  });
  return loading;
}

/** Whether this computer can draw with WebGL. */
export function webglAvailable() {
  try {
    const canvas = document.createElement('canvas');
    const gl = canvas.getContext('webgl2') ?? canvas.getContext('webgl');
    if (!gl) return false;
    // Give the test context back at once.
    gl.getExtension('WEBGL_lose_context')?.loseContext();
    return true;
  } catch {
    return false;
  }
}

/**
 * Reads the GLB in `bytes` (an ArrayBuffer) and draws it in `container`,
 * on `background`. Resolves to the view. `onLost` runs if the browser
 * takes the view's WebGL away, such as after a graphics driver reset.
 */
export async function createView(container, bytes, background, onLost) {
  const { THREE, GLTFLoader } = await loadThree();
  const gltf = await new Promise((resolve, reject) => {
    new GLTFLoader().parse(bytes, '', resolve, (error) => reject(new Error(String(error?.message ?? error))));
  });
  return new View3d(THREE, container, gltf, background, onLost);
}

const DEGREE = Math.PI / 180;
/** The starting view looks down at the front at this angle. */
const START_ELEVATION = 45 * DEGREE;
/** Top and Bottom stop just short of straight down or up. */
const STRAIGHT = 89.9 * DEGREE;
/** How far one arrow key turns the model. */
const TURN_STEP = 10 * DEGREE;
/** How far a drag of one pixel turns the model. */
const TURN_PER_PIXEL = 0.5 * DEGREE;
/** How far Shift and an arrow key pan, as a share of the view. */
const PAN_STEP = 0.1;
/** How much + and - zoom. */
const ZOOM_STEP = 1.5;
/** The camera's field of view, in degrees. */
const FIELD_OF_VIEW = 30;

class View3d {
  constructor(THREE, container, gltf, background, onLost) {
    const model = gltf.scene;
    this.THREE = THREE;
    this.container = container;
    this.model = model;
    // The GLB's own mesh names: the model's, from its file, and the
    // board's, such as board_pad.
    this.meshNames = (gltf.parser?.json?.meshes ?? []).map((mesh) => mesh.name ?? '');
    this.background = new THREE.Color(background);

    this.renderer = new THREE.WebGLRenderer({ antialias: true });
    this.renderer.setPixelRatio(window.devicePixelRatio || 1);
    this.canvas = this.renderer.domElement;
    this.canvas.className = 'view3d-canvas';
    this.canvas.setAttribute('aria-hidden', 'true');
    this.lost = (event) => {
      event.preventDefault();
      onLost?.();
    };
    this.canvas.addEventListener('webglcontextlost', this.lost);
    container.appendChild(this.canvas);

    this.scene = new THREE.Scene();
    this.scene.background = this.background;
    this.scene.add(model);
    // Soft, even light with no shadows: light from the sky and the ground,
    // and two lights that move with the camera, so every side is lit.
    this.scene.add(new THREE.HemisphereLight(0xffffff, 0x8d8d8d, 1.6));
    this.camera = new THREE.PerspectiveCamera(FIELD_OF_VIEW, 1, 1e-6, 10);
    const key = new THREE.DirectionalLight(0xffffff, 1.4);
    key.position.set(-0.5, 1, 1);
    const fill = new THREE.DirectionalLight(0xffffff, 0.6);
    fill.position.set(0.7, -0.3, 1);
    this.camera.add(key, key.target, fill, fill.target);
    key.target.position.set(0, 0, -1);
    fill.target.position.set(0, 0, -1);
    this.scene.add(this.camera);

    // Turn around the model's centre; fit its whole size.
    const box = new THREE.Box3().setFromObject(model);
    this.centre = box.isEmpty() ? new THREE.Vector3() : box.getCenter(new THREE.Vector3());
    const size = box.isEmpty() ? 0.001 : box.getSize(new THREE.Vector3()).length() / 2;
    this.radius = Math.max(size, 1e-5);
    this.target = new THREE.Vector3();
    this.reset();

    this.listen();
    this.resize();
    // The journeys read what is drawn through the view area.
    container.view3dInfo = () => this.info();
  }

  // ---- Views ---------------------------------------------------------------

  /** The starting view: down at the front, 45 degrees up. */
  reset() {
    this.show(0, START_ELEVATION);
  }

  /** Straight down at the top, as the 2D view shows it. */
  top() {
    this.show(0, STRAIGHT);
  }

  /** Straight up at the bottom, mirrored as KiCad shows it. */
  bottom() {
    this.show(Math.PI, -STRAIGHT);
  }

  show(azimuth, elevation) {
    this.azimuth = azimuth;
    this.elevation = elevation;
    this.target.copy(this.centre);
    this.distance = this.fittingDistance();
    this.place();
  }

  /** How far away the camera sees the whole model. */
  fittingDistance() {
    const vertical = (FIELD_OF_VIEW / 2) * DEGREE;
    const horizontal = Math.atan(Math.tan(vertical) * Math.max(this.camera.aspect, 1e-3));
    return (this.radius * 1.15) / Math.sin(Math.min(vertical, horizontal));
  }

  /** Puts the camera where the angles, distance and target say. */
  place() {
    const { THREE } = this;
    this.elevation = Math.max(-STRAIGHT, Math.min(STRAIGHT, this.elevation));
    const flat = Math.cos(this.elevation);
    const offset = new THREE.Vector3(
      flat * Math.sin(this.azimuth),
      Math.sin(this.elevation),
      flat * Math.cos(this.azimuth),
    ).multiplyScalar(this.distance);
    this.camera.position.copy(this.target).add(offset);
    this.camera.up.set(0, 1, 0);
    this.camera.lookAt(this.target);
    this.camera.near = this.distance / 200;
    this.camera.far = this.distance * 20 + this.radius * 4;
    this.camera.updateProjectionMatrix();
    this.render();
  }

  turn(right, up) {
    this.azimuth -= right;
    this.elevation += up;
    this.place();
  }

  /** Moves the view by `dx`, `dy` pixels, as a drag would. */
  pan(dx, dy) {
    const { THREE } = this;
    const height = Math.max(this.canvas.clientHeight, 1);
    const perPixel = (2 * this.distance * Math.tan((FIELD_OF_VIEW / 2) * DEGREE)) / height;
    const right = new THREE.Vector3().setFromMatrixColumn(this.camera.matrix, 0);
    const up = new THREE.Vector3().setFromMatrixColumn(this.camera.matrix, 1);
    this.target.addScaledVector(right, -dx * perPixel).addScaledVector(up, dy * perPixel);
    this.place();
  }

  /**
   * Zooms in by `factor` (more than 1 zooms in) at the pixel `x`, `y` of
   * the view, which stays where it is; at the centre with no pixel.
   */
  zoom(factor, x, y) {
    const { THREE } = this;
    const nearest = this.radius * 0.02;
    const farthest = this.radius * 60;
    const distance = Math.max(nearest, Math.min(farthest, this.distance / factor));
    const scale = distance / this.distance;
    let at = this.target.clone();
    if (x !== undefined) {
      // The point under the pointer on the plane through the target.
      const width = Math.max(this.canvas.clientWidth, 1);
      const height = Math.max(this.canvas.clientHeight, 1);
      const ray = new THREE.Raycaster();
      ray.setFromCamera(new THREE.Vector2((x / width) * 2 - 1, 1 - (y / height) * 2), this.camera);
      const facing = this.camera.getWorldDirection(new THREE.Vector3());
      const plane = new THREE.Plane().setFromNormalAndCoplanarPoint(facing, this.target);
      at = ray.ray.intersectPlane(plane, new THREE.Vector3()) ?? at;
    }
    // Scaling the camera and target about that point keeps it in place.
    this.target.sub(at).multiplyScalar(scale).add(at);
    this.distance = distance;
    this.place();
  }

  render() {
    this.renderer.render(this.scene, this.camera);
  }

  resize() {
    const width = Math.max(this.container.clientWidth, 1);
    const height = Math.max(this.container.clientHeight, 1);
    this.renderer.setSize(width, height, false);
    this.camera.aspect = width / height;
    this.place();
  }

  // ---- Pointer, wheel and keys --------------------------------------------

  listen() {
    const container = this.container;
    let drag = null;
    this.handlers = {
      pointerdown: (event) => {
        if (event.button > 2) return;
        event.preventDefault();
        container.focus();
        drag = { id: event.pointerId, x: event.clientX, y: event.clientY, turn: event.button === 0 };
        container.setPointerCapture?.(event.pointerId);
        container.classList.add('dragging');
      },
      pointermove: (event) => {
        if (!drag || drag.id !== event.pointerId) return;
        if (event.buttons === 0) {
          this.handlers.pointerup(event);
          return;
        }
        const dx = event.clientX - drag.x;
        const dy = event.clientY - drag.y;
        drag.x = event.clientX;
        drag.y = event.clientY;
        if (drag.turn) this.turn(dx * TURN_PER_PIXEL, dy * TURN_PER_PIXEL);
        else this.pan(dx, dy);
      },
      pointerup: (event) => {
        if (drag && drag.id === event.pointerId) {
          drag = null;
          container.classList.remove('dragging');
        }
      },
      // The right button pans, so it opens no menu.
      contextmenu: (event) => event.preventDefault(),
      // No autoscroll with the middle button.
      mousedown: (event) => {
        if (event.button === 1) event.preventDefault();
      },
      wheel: (event) => {
        event.preventDefault();
        // Lines and pages are about 40 and 800 pixels.
        const unit = event.deltaMode === 1 ? 40 : event.deltaMode === 2 ? 800 : 1;
        const factor = Math.exp(Math.max(-1, Math.min(1, -event.deltaY * unit * 0.002)));
        const rect = this.canvas.getBoundingClientRect();
        this.zoom(factor, event.clientX - rect.left, event.clientY - rect.top);
      },
      keydown: (event) => {
        // Ctrl, Alt and the Windows key with these keys belong to others.
        if (event.ctrlKey || event.altKey || event.metaKey) return;
        const width = this.canvas.clientWidth;
        const height = this.canvas.clientHeight;
        const step = Math.min(width, height) * PAN_STEP;
        const arrows = {
          ArrowLeft: [-1, 0],
          ArrowRight: [1, 0],
          ArrowUp: [0, -1],
          ArrowDown: [0, 1],
        };
        const arrow = arrows[event.key];
        if (arrow && event.shiftKey) this.pan(-arrow[0] * step, -arrow[1] * step);
        else if (arrow) this.turn(arrow[0] * TURN_STEP, -arrow[1] * TURN_STEP);
        else if (event.key === '+' || event.key === '=') this.zoom(ZOOM_STEP);
        else if (event.key === '-' || event.key === '_') this.zoom(1 / ZOOM_STEP);
        else if (event.key === '0') this.reset();
        else return;
        event.preventDefault();
      },
    };
    for (const [name, handler] of Object.entries(this.handlers)) {
      container.addEventListener(name, handler, name === 'wheel' ? { passive: false } : undefined);
    }
    for (const name of ['pointercancel', 'lostpointercapture']) {
      container.addEventListener(name, this.handlers.pointerup);
    }
    this.observer = new ResizeObserver(() => this.resize());
    this.observer.observe(container);
  }

  // ---- For tests and the page ---------------------------------------------

  /**
   * What is drawn: the meshes, by name, and how many pixels differ from
   * the background. It draws again first, so the pixels can be read.
   */
  info() {
    let meshes = 0;
    this.model.traverse((object) => {
      if (object.isMesh) meshes += 1;
    });
    this.render();
    const gl = this.renderer.getContext();
    const width = gl.drawingBufferWidth;
    const height = gl.drawingBufferHeight;
    const pixels = new Uint8Array(width * height * 4);
    gl.readPixels(0, 0, width, height, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    const back = this.background.clone().convertLinearToSRGB();
    const [r, g, b] = [back.r, back.g, back.b].map((value) => Math.round(value * 255));
    let drawn = 0;
    for (let at = 0; at < pixels.length; at += 4) {
      const differs =
        Math.abs(pixels[at] - r) > 8 || Math.abs(pixels[at + 1] - g) > 8 || Math.abs(pixels[at + 2] - b) > 8;
      if (differs) drawn += 1;
    }
    return {
      meshes,
      meshNames: this.meshNames,
      width,
      height,
      drawn,
      azimuth: this.azimuth / DEGREE,
      elevation: this.elevation / DEGREE,
      distance: this.distance,
      fitting: this.fittingDistance(),
      target: this.target.toArray(),
    };
  }

  /** Frees the GPU memory and removes every listener. */
  dispose() {
    for (const [name, handler] of Object.entries(this.handlers ?? {})) {
      this.container.removeEventListener(name, handler);
    }
    for (const name of ['pointercancel', 'lostpointercapture']) {
      this.container.removeEventListener(name, this.handlers?.pointerup);
    }
    this.container.classList.remove('dragging');
    delete this.container.view3dInfo;
    this.observer?.disconnect();
    this.scene.traverse((object) => {
      object.geometry?.dispose();
      const materials = Array.isArray(object.material) ? object.material : [object.material];
      for (const material of materials) {
        if (!material) continue;
        for (const value of Object.values(material)) {
          if (value?.isTexture) value.dispose();
        }
        material.dispose();
      }
    });
    this.scene.clear();
    // Freeing the context below is no loss to report.
    this.canvas.removeEventListener('webglcontextlost', this.lost);
    this.renderer.dispose();
    this.renderer.forceContextLoss();
    this.canvas.remove();
  }
}

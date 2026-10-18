# The 3D view always turns around the model's centre

The wheel zooms at the pointer, but a drag always turns the model around its own centre. The user asked for this on 2026-10-16 (kicad-parts-4588): you can zoom into a corner and still turn the whole part steadily.

Most 3D viewers, such as three.js's OrbitControls, zoom at the pointer by moving the point the model turns around. A pan does the same. Later turns then swing the model off screen, and people lose it. So our turning point never moves. The camera stays at one distance outside the model, and zoom narrows its view, like a zoom lens. Pan and zoom at the pointer slide the camera sideways in its own plane. The model's centre then stays at the same place on screen while it turns.

KiCad's 3D viewer turns and pans the same way, and its closest zoom is also 50 times. But KiCad zooms by moving the camera nearer, so close up it can pass into a part. A lens can't. Close up, a lens gives a nearly flat view, like KiCad's orthographic mode, which engineers use to check how a part sits anyway.

## Consequences

- The camera can never go inside the model, and nothing is ever cut off, because it never moves nearer.
- Zoom runs from a quarter to 50 times the zoom at which the model's sphere fits the view's height.
- The middle of the view always shows the model: the line it looks along always meets the model's box. A pan stops at the box's edge as the camera sees it. A turn that would carry the middle off the box slides the view just enough to stay on it; otherwise the centre stays put.
- The rules are plain maths in `apps/desktop-ui/js/view3d-camera.js`, with unit tests that the gate runs. Journeys 20 and 21 check them in the app.

Mulch records: mx-46edf2, mx-4e9bfa, mx-b61ecb.

# The 3D view always turns around the model's centre

The wheel zooms at the pointer, but a drag always turns the model around its own centre. The user asked for this on 2026-10-16 (kicad-parts-4588): you can zoom into a corner and still turn the whole part steadily.

Most 3D viewers, such as three.js's OrbitControls, zoom at the pointer by moving the point the model turns around. A pan does the same. Later turns then swing the model off screen, and people lose it. So our turning point never moves. The camera stays at one distance outside the model, and zoom narrows its view, like a zoom lens. Pan and zoom at the pointer slide the camera sideways in its own plane. The model's centre then stays at the same place on screen while it turns.

## Consequences

- The camera can never go inside the model, and nothing is ever cut off, because it never moves nearer.
- Zoom has fixed limits: from a quarter to 50 times the starting view. Pan keeps the middle of the view over the model's box.
- Zoomed far in, the view is nearly flat, like a long lens. That suits looking at pads and pins.
- The rules are plain maths in `apps/desktop-ui/js/view3d-camera.js`, with unit tests that the gate runs. Journeys 20 and 21 check them in the app.

Mulch records: mx-46edf2, mx-f8b156, mx-b61ecb.

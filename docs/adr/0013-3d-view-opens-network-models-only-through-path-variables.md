# The 3D view opens network models only through a path variable

A footprint's 3D model link can name any path. When a link names a network computer, Windows may send the person's sign-in details to it. So the 3D view opens only drive paths and paths that start with one slash, plus folders from path variables the person set themselves. A path such as `/./x.step` or `/C:/x.step` stays on this computer, so the app looks for it and says when the file is missing. KiCad opens direct network links, and we refuse them on purpose. Mapped network drives still work.

The rule is in `crates/catalog-browser/src/model3d.rs`.

Mulch records: mx-cc7bc7.

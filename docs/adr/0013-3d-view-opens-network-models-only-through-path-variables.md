# The app opens network models only through a path variable

A footprint's 3D model link can name any path. When a link names a network computer, Windows may send the person's sign-in details to it. So the app opens a model file only at a drive path, such as `C:\models\x.step`, or in a folder from a path variable the person set themselves. KiCad opens direct network links, and we refuse them on purpose. Mapped network drives still work.

This applies to the 3D view and to copying a model from an installed library. Both use one rule, `openable_path` in `crates/catalog-browser/src/model3d.rs`. Windows follows symbolic links, so before opening a file, `checked_model` checks every link on its path, without following it, and refuses one that leads off this computer's drives.

A path with one slash and no drive, such as `/models/x.step` or `/./x.step`, means the root of a drive. Windows takes that drive from the app's current folder. The app opens such a path only when it lands on a drive letter, and then says when the file is missing. If the app started in a network folder, the path would land on a network share, so it is refused.

Mulch records: mx-cc7bc7.

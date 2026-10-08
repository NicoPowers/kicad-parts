//! Turns a catalog library reference such as `g-pas:R_US` into a concrete
//! library on disk. Callers pass the reference; this module owns where
//! libraries live and in which order they are searched.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// A `LIBRARY:ITEM` reference, as stored in the catalog.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LibRef {
    pub library: String,
    pub item: String,
}

impl LibRef {
    pub fn parse(text: &str) -> Result<Self, ResolveError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ResolveError::Unassigned);
        }
        let malformed = || ResolveError::Malformed(text.to_owned());
        let (library, item) = text.split_once(':').ok_or_else(malformed)?;
        // Both halves become file names, so refuse anything that could walk
        // out of a library directory.
        let unsafe_name = |name: &str| {
            name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', ':', '\0'])
        };
        if unsafe_name(library) || unsafe_name(item) {
            return Err(malformed());
        }
        Ok(Self {
            library: library.to_owned(),
            item: item.to_owned(),
        })
    }
}

impl fmt::Display for LibRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.library, self.item)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LibraryOrigin {
    /// A library stored in this repository (`symbols/`, `footprints/`).
    Project,
    /// A library bundled with the installed KiCad.
    KicadBundled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedSymbol {
    pub lib_ref: LibRef,
    /// The `.kicad_sym` library file.
    pub library: PathBuf,
    pub origin: LibraryOrigin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedFootprint {
    pub lib_ref: LibRef,
    /// The `.pretty` library directory.
    pub library: PathBuf,
    /// The footprint's `.kicad_mod` file inside `library`.
    pub file: PathBuf,
    pub origin: LibraryOrigin,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolveError {
    /// The part has no symbol or footprint assigned.
    Unassigned,
    Malformed(String),
    LibraryNotFound {
        lib_ref: String,
        searched: Vec<PathBuf>,
    },
    FootprintNotFound {
        lib_ref: String,
        library: PathBuf,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unassigned => f.write_str("no library reference is assigned"),
            Self::Malformed(text) => {
                write!(f, "`{text}` is not a LIBRARY:NAME library reference")
            }
            Self::LibraryNotFound { lib_ref, searched } => {
                write!(f, "library for `{lib_ref}` was not found; looked for")?;
                for (n, path) in searched.iter().enumerate() {
                    let separator = if n == 0 { " " } else { ", " };
                    write!(f, "{separator}{}", path.display())?;
                }
                Ok(())
            }
            Self::FootprintNotFound { lib_ref, library } => write!(
                f,
                "footprint `{lib_ref}` is not in library {}",
                library.display()
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

/// Searches project libraries first, then KiCad's bundled libraries.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryResolver {
    project_root: Option<PathBuf>,
    kicad_share: Option<PathBuf>,
}

impl LibraryResolver {
    /// `project_root` holds `symbols/` and `footprints/`; `kicad_share` is
    /// the installed KiCad data directory, if known.
    pub fn new(project_root: Option<PathBuf>, kicad_share: Option<PathBuf>) -> Self {
        Self {
            project_root,
            kicad_share,
        }
    }

    pub fn symbol(&self, reference: &str) -> Result<ResolvedSymbol, ResolveError> {
        let lib_ref = LibRef::parse(reference)?;
        let file_name = format!("{}.kicad_sym", lib_ref.library);
        let (library, origin) = self.find_library("symbols", &file_name, &lib_ref)?;
        Ok(ResolvedSymbol {
            lib_ref,
            library,
            origin,
        })
    }

    /// Resolves a catalog footprint field. Like KiCad database libraries,
    /// the field may list several footprints separated by `;`; the first is
    /// the part's default footprint and is the one resolved.
    pub fn footprint(&self, field: &str) -> Result<ResolvedFootprint, ResolveError> {
        let reference = footprint_choices(field)
            .into_iter()
            .next()
            .unwrap_or_default();
        let lib_ref = LibRef::parse(reference)?;
        let dir_name = format!("{}.pretty", lib_ref.library);
        let (library, origin) = self.find_library("footprints", &dir_name, &lib_ref)?;
        let Some(file) = exact_entry(&library, &format!("{}.kicad_mod", lib_ref.item))
            .filter(|file| file.is_file())
        else {
            return Err(ResolveError::FootprintNotFound {
                lib_ref: lib_ref.to_string(),
                library,
            });
        };
        Ok(ResolvedFootprint {
            lib_ref,
            library,
            file,
            origin,
        })
    }

    fn find_library(
        &self,
        kind: &str,
        name: &str,
        lib_ref: &LibRef,
    ) -> Result<(PathBuf, LibraryOrigin), ResolveError> {
        let candidates = [
            (self.project_root.as_deref(), LibraryOrigin::Project),
            (self.kicad_share.as_deref(), LibraryOrigin::KicadBundled),
        ];
        let mut searched = Vec::new();
        for (root, origin) in candidates {
            let Some(root) = root else { continue };
            let dir = root.join(kind);
            if let Some(path) = exact_entry(&dir, name) {
                return Ok((path, origin));
            }
            searched.push(dir.join(name));
        }
        Err(ResolveError::LibraryNotFound {
            lib_ref: lib_ref.to_string(),
            searched,
        })
    }
}

/// The `;`-separated footprint choices in a catalog field, default first.
pub fn footprint_choices(field: &str) -> Vec<&str> {
    field
        .split(';')
        .map(str::trim)
        .filter(|choice| !choice.is_empty())
        .collect()
}

/// `dir/name` only if an entry with exactly that name exists. KiCad matches
/// library and footprint names case-sensitively, but Windows and macOS file
/// systems usually do not, so `Path::exists` would accept `device:R` for
/// `Device.kicad_sym`.
fn exact_entry(dir: &Path, name: &str) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|entry| entry.file_name() == name)
        .map(|entry| entry.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_rejects_references() {
        let lib_ref = LibRef::parse(" g-pas:R_US ").unwrap();
        assert_eq!(lib_ref.library, "g-pas");
        assert_eq!(lib_ref.item, "R_US");
        assert_eq!(LibRef::parse(""), Err(ResolveError::Unassigned));
        for bad in [
            "R_US", "g-pas:", ":R", "../x:R", "lib:a/b", "lib:..", "a:b:c",
        ] {
            assert!(
                matches!(LibRef::parse(bad), Err(ResolveError::Malformed(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn project_libraries_shadow_bundled_libraries() {
        let project = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        for root in [project.path(), share.path()] {
            fs::create_dir_all(root.join("symbols")).unwrap();
            fs::write(root.join("symbols").join("Device.kicad_sym"), b"").unwrap();
        }
        fs::write(share.path().join("symbols").join("Only.kicad_sym"), b"").unwrap();
        let resolver = LibraryResolver::new(
            Some(project.path().to_path_buf()),
            Some(share.path().to_path_buf()),
        );
        let device = resolver.symbol("Device:R").unwrap();
        assert_eq!(device.origin, LibraryOrigin::Project);
        let only = resolver.symbol("Only:X").unwrap();
        assert_eq!(only.origin, LibraryOrigin::KicadBundled);
        match resolver.symbol("Nope:X") {
            Err(ResolveError::LibraryNotFound { searched, .. }) => assert_eq!(searched.len(), 2),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn footprint_must_exist_inside_its_library() {
        let project = tempfile::tempdir().unwrap();
        let pretty = project.path().join("footprints").join("g-res.pretty");
        fs::create_dir_all(&pretty).unwrap();
        fs::write(pretty.join("R_0603.kicad_mod"), b"").unwrap();
        let resolver = LibraryResolver::new(Some(project.path().to_path_buf()), None);
        let found = resolver.footprint("g-res:R_0603").unwrap();
        assert_eq!(found.file, pretty.join("R_0603.kicad_mod"));
        let listed = resolver
            .footprint(" g-res:R_0603 ; g-res:R_0603_Hand")
            .unwrap();
        assert_eq!(listed.lib_ref.item, "R_0603");
        assert_eq!(footprint_choices("a:b;;c:d ;"), vec!["a:b", "c:d"]);
        // Names match case-sensitively, as in KiCad, even where the file
        // system does not.
        assert!(matches!(
            resolver.footprint("g-res:r_0603"),
            Err(ResolveError::FootprintNotFound { .. })
        ));
        assert!(matches!(
            resolver.footprint("G-RES:R_0603"),
            Err(ResolveError::LibraryNotFound { .. })
        ));
        assert!(matches!(
            resolver.footprint("g-res:R_0402"),
            Err(ResolveError::FootprintNotFound { .. })
        ));
    }
}

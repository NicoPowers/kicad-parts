//! Turns a catalog library reference such as `g-res:R_US` into a concrete
//! library on disk. Callers pass the reference; this module owns where
//! libraries live and in which order they are searched.
//!
//! A reference's nickname is looked up in a library source profile
//! ([`SourceProfile`]): our own layout by default, or a client repo's,
//! whose libraries come from the repo's own lib tables (kicad-parts-e9a3,
//! which took over kicad-parts-e83f). So a nickname that differs from its
//! file name, such as `Acme_FP` for `lib/Acme.pretty`, finds the library
//! KiCad would. Then KiCad's bundled libraries are searched.

use crate::lib_table::TableKind;
use crate::library_source::SourceProfile;
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
    /// A library of the project's library source: by default this
    /// repository's own (`symbols/`, `footprints/`).
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

/// Searches the project's library source first, then KiCad's bundled
/// libraries.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryResolver {
    /// The project's folder and where its libraries are in it.
    project: Option<(PathBuf, SourceProfile)>,
    kicad_share: Option<PathBuf>,
}

impl LibraryResolver {
    /// `project_root` holds our own layout (`symbols/`, `footprints/`, the
    /// default profile); `kicad_share` is the installed KiCad data
    /// directory, if known.
    pub fn new(project_root: Option<PathBuf>, kicad_share: Option<PathBuf>) -> Self {
        Self {
            project: project_root.map(|root| (root, SourceProfile::own())),
            kicad_share,
        }
    }

    /// Resolves through `source`, the layout of the library repo files in
    /// `root`, before KiCad's bundled libraries in `kicad_share`.
    pub fn with_source(root: PathBuf, source: SourceProfile, kicad_share: Option<PathBuf>) -> Self {
        Self {
            project: Some((root, source)),
            kicad_share,
        }
    }

    pub fn symbol(&self, reference: &str) -> Result<ResolvedSymbol, ResolveError> {
        let lib_ref = LibRef::parse(reference)?;
        let (library, origin) = self.find_library(TableKind::Symbol, &lib_ref)?;
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
        let (library, origin) = self.find_library(TableKind::Footprint, &lib_ref)?;
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
        kind: TableKind,
        lib_ref: &LibRef,
    ) -> Result<(PathBuf, LibraryOrigin), ResolveError> {
        // KiCad's data folder keeps its libraries as our layout does.
        let bundled = SourceProfile::own();
        let candidates = [
            self.project
                .as_ref()
                .map(|(root, source)| (root.as_path(), source, LibraryOrigin::Project)),
            self.kicad_share
                .as_deref()
                .map(|share| (share, &bundled, LibraryOrigin::KicadBundled)),
        ];
        let mut searched = Vec::new();
        for (root, source, origin) in candidates.into_iter().flatten() {
            let Some(path) = source.library_path(kind, &lib_ref.library) else {
                continue;
            };
            if let Some(found) = exact_path(root, &path) {
                return Ok((found, origin));
            }
            searched.push(native_path(root, &path));
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

/// The repo path `path` under `root`, only if every part of it exists with
/// exactly that name (see [`exact_entry`]).
fn exact_path(root: &Path, path: &str) -> Option<PathBuf> {
    path.split('/')
        .try_fold(root.to_path_buf(), |folder, name| {
            exact_entry(&folder, name)
        })
}

/// The repo path `path` under `root`, with this system's separators.
fn native_path(root: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(root.to_path_buf(), |folder, name| folder.join(name))
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
        let lib_ref = LibRef::parse(" g-res:R_US ").unwrap();
        assert_eq!(lib_ref.library, "g-res");
        assert_eq!(lib_ref.item, "R_US");
        assert_eq!(LibRef::parse(""), Err(ResolveError::Unassigned));
        for bad in [
            "R_US", "g-res:", ":R", "../x:R", "lib:a/b", "lib:..", "a:b:c",
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
        let pretty = project
            .path()
            .join("footprints")
            .join("g-Resistor_SMD.pretty");
        fs::create_dir_all(&pretty).unwrap();
        fs::write(pretty.join("R_0603.kicad_mod"), b"").unwrap();
        let resolver = LibraryResolver::new(Some(project.path().to_path_buf()), None);
        let found = resolver.footprint("g-Resistor_SMD:R_0603").unwrap();
        assert_eq!(found.file, pretty.join("R_0603.kicad_mod"));
        let listed = resolver
            .footprint(" g-Resistor_SMD:R_0603 ; g-Resistor_SMD:R_0603_Hand")
            .unwrap();
        assert_eq!(listed.lib_ref.item, "R_0603");
        assert_eq!(footprint_choices("a:b;;c:d ;"), vec!["a:b", "c:d"]);
        // Names match case-sensitively, as in KiCad, even where the file
        // system does not.
        assert!(matches!(
            resolver.footprint("g-Resistor_SMD:r_0603"),
            Err(ResolveError::FootprintNotFound { .. })
        ));
        assert!(matches!(
            resolver.footprint("G-RES:R_0603"),
            Err(ResolveError::LibraryNotFound { .. })
        ));
        assert!(matches!(
            resolver.footprint("g-Resistor_SMD:R_0402"),
            Err(ResolveError::FootprintNotFound { .. })
        ));
    }

    #[test]
    fn a_client_source_resolves_nicknames_through_its_lib_tables() {
        use crate::library_source::{LibraryList, SourceLibrary};
        let repo = tempfile::tempdir().unwrap();
        let pretty = repo.path().join("lib").join("Acme.pretty");
        fs::create_dir_all(&pretty).unwrap();
        fs::write(pretty.join("R_0603.kicad_mod"), b"").unwrap();
        // As the repo's fp-lib-table names it: a nickname unlike the folder.
        let source = SourceProfile {
            footprints: LibraryList::Listed {
                libraries: vec![SourceLibrary {
                    nickname: "Acme_FP".into(),
                    path: "lib/Acme.pretty".into(),
                }],
            },
            ..SourceProfile::own()
        };
        let resolver = LibraryResolver::with_source(repo.path().to_path_buf(), source, None);
        let found = resolver.footprint("Acme_FP:R_0603").unwrap();
        assert_eq!(found.file, pretty.join("R_0603.kicad_mod"));
        assert_eq!(found.origin, LibraryOrigin::Project);
        // The file name is no nickname, and a table row's path matches
        // with exact case, even where the file system does not.
        assert!(matches!(
            resolver.footprint("Acme:R_0603"),
            Err(ResolveError::LibraryNotFound { .. })
        ));
        fs::rename(&pretty, repo.path().join("lib").join("acme.pretty")).unwrap();
        assert!(matches!(
            resolver.footprint("Acme_FP:R_0603"),
            Err(ResolveError::LibraryNotFound { .. })
        ));
    }
}

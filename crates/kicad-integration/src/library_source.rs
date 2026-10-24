//! A library source profile: where one library repo keeps its KiCad
//! libraries (kicad-parts-e9a3, ADR 0022).
//!
//! Every client lays out its library repo its own way. Models may sit next
//! to footprints or in a folder of their own, lib tables may be in the
//! repo or not, and path variables have the client's own names. A profile
//! says, for one repo:
//!
//! - its symbol and footprint libraries, each by the nickname boards use
//!   and its path in the repo;
//! - the folders that hold its 3D models;
//! - its path variables, each with the repo folder it points to;
//! - any `.kicad_dbl` files;
//! - a short prefix that tells sources apart inside the app.
//!
//! Nicknames are never renamed: a client's boards store them, so they stay
//! exactly as the repo's lib tables (or file names) give them. The prefix
//! is only for the app (ADR 0022).
//!
//! Our own repo's layout (`symbols/`, `footprints/`, `3d-models/` and
//! `${GITPLM_PARTS}`) is the default profile, [`SourceProfile::own`]. The
//! library check, the import, Add part, the Libraries browser and the
//! resolver read our libraries through it.
//!
//! Paths in a profile are repo paths: relative, with `/`, matched with
//! exact case, so a profile means the same on Windows, macOS and Linux.

use crate::lib_table::TableKind;
use serde::{Deserialize, Serialize};

/// One library of a source: the nickname boards use, such as
/// `Acme_Passives`, and its file or folder in the repo.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SourceLibrary {
    pub nickname: String,
    /// Such as `lib/Acme_Passives.kicad_sym` or `lib/Acme.pretty`.
    pub path: String,
}

/// Which libraries a source has, of one kind.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "rule", rename_all = "snake_case")]
pub enum LibraryList {
    /// Each library by name, as a scan found them.
    Listed { libraries: Vec<SourceLibrary> },
    /// Every library directly in `folder`, its nickname its file name
    /// without the extension: our own layout, where a new library needs
    /// no change to the profile.
    EveryLibraryIn { folder: String },
}

/// One path variable of a source, such as `ACME_3D`, and the repo folder
/// it must point to: blank for the repo's top folder, `None` when the
/// scan could not tell.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SourceVariable {
    pub name: String,
    pub folder: Option<String>,
}

/// Where one library repo keeps its KiCad libraries.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceProfile {
    /// Tells this source apart inside the app, such as `AC`. Never part of
    /// a name KiCad or a board sees, except for libraries the app makes.
    pub prefix: String,
    pub symbols: LibraryList,
    pub footprints: LibraryList,
    /// The repo folders that hold 3D models, such as `3d-models`.
    pub model_folders: Vec<String>,
    pub path_variables: Vec<SourceVariable>,
    /// The repo's `.kicad_dbl` files.
    #[serde(default)]
    pub database_libraries: Vec<String>,
    /// A folder holding one `<footprint library>.3dshapes` folder per
    /// footprint library, where each footprint's own 3D model goes. Only
    /// our own layout has one yet; kicad-parts-02fb works out such rules
    /// for client repos.
    #[serde(default)]
    pub models_by_library: Option<String>,
}

/// The folders, variable and prefix of our own layout.
pub mod own {
    /// Our symbol libraries: `symbols/g-<category>.kicad_sym`.
    pub const SYMBOL_FOLDER: &str = "symbols";
    /// Our footprint libraries: `footprints/g-<package>.pretty`.
    pub const FOOTPRINT_FOLDER: &str = "footprints";
    /// Our 3D models: `3d-models/g-<package>.3dshapes/<footprint>.step`.
    pub const MODEL_FOLDER: &str = "3d-models";
    /// Points to the repo's top folder in every link to our 3D models.
    pub const VARIABLE: &str = "GITPLM_PARTS";
    /// Our libraries are named `g-...`.
    pub const PREFIX: &str = "g";
}

/// Where a 3D model link leads, read through a source's path variables.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelLink {
    /// A file in the repo, such as `3d-models/g-SO.3dshapes/SOIC-8.step`.
    /// `in_model_folder` when the path is inside one of the source's model
    /// folders.
    InRepo { path: String, in_model_folder: bool },
    /// Through one of the source's variables, but not a plain path inside
    /// the repo: it climbs out with `..` or names a drive.
    Unsafe,
    /// Through none of the source's variables.
    Elsewhere,
}

impl SourceProfile {
    /// Our own repo's layout: the default profile.
    ///
    /// Symbol libraries are every `.kicad_sym` file in `symbols/`,
    /// footprint libraries every `.pretty` folder in `footprints/`, and 3D
    /// models are linked as `${GITPLM_PARTS}/3d-models/...`.
    pub fn own() -> Self {
        Self {
            prefix: own::PREFIX.to_owned(),
            symbols: LibraryList::EveryLibraryIn {
                folder: own::SYMBOL_FOLDER.to_owned(),
            },
            footprints: LibraryList::EveryLibraryIn {
                folder: own::FOOTPRINT_FOLDER.to_owned(),
            },
            model_folders: vec![own::MODEL_FOLDER.to_owned()],
            path_variables: vec![SourceVariable {
                name: own::VARIABLE.to_owned(),
                folder: Some(String::new()),
            }],
            database_libraries: Vec::new(),
            models_by_library: Some(own::MODEL_FOLDER.to_owned()),
        }
    }

    /// The libraries of `kind`.
    pub fn libraries(&self, kind: TableKind) -> &LibraryList {
        match kind {
            TableKind::Symbol => &self.symbols,
            TableKind::Footprint => &self.footprints,
        }
    }

    /// Whether `nickname` is one the app made for this source: it starts
    /// with the prefix and a dash, such as our `g-res`.
    pub fn is_own_name(&self, nickname: &str) -> bool {
        nickname
            .strip_prefix(&self.prefix)
            .is_some_and(|rest| rest.starts_with('-'))
    }

    /// The repo path of the library `nickname` of `kind`, such as
    /// `symbols/g-res.kicad_sym`; `None` if the source has no such
    /// library, or the nickname could name another folder. A symbol
    /// library found by its folder is a `.kicad_sym` file.
    pub fn library_path(&self, kind: TableKind, nickname: &str) -> Option<String> {
        match self.libraries(kind) {
            LibraryList::Listed { libraries } => libraries
                .iter()
                .find(|library| library.nickname == nickname)
                .map(|library| library.path.clone()),
            LibraryList::EveryLibraryIn { folder } => {
                let plain = !nickname.is_empty()
                    && nickname != "."
                    && nickname != ".."
                    && !nickname.contains(['/', '\\', ':', '\0']);
                plain.then(|| {
                    let extension = match kind {
                        TableKind::Symbol => "kicad_sym",
                        TableKind::Footprint => "pretty",
                    };
                    join(folder, &format!("{nickname}.{extension}"))
                })
            }
        }
    }

    /// The nickname of the library at the repo path `path`, if it is one
    /// of the source's libraries of `kind`. Found by folder, a symbol
    /// library may also be a `.kicad_symdir` folder (KiCad 10).
    pub fn nickname_of(&self, kind: TableKind, path: &str) -> Option<String> {
        match self.libraries(kind) {
            LibraryList::Listed { libraries } => libraries
                .iter()
                .find(|library| library.path == path)
                .map(|library| library.nickname.clone()),
            LibraryList::EveryLibraryIn { folder } => {
                let name = if folder.is_empty() {
                    path
                } else {
                    path.strip_prefix(folder.as_str())?.strip_prefix('/')?
                };
                if name.contains('/') {
                    return None;
                }
                let extensions: &[&str] = match kind {
                    TableKind::Symbol => &[".kicad_sym", ".kicad_symdir"],
                    TableKind::Footprint => &[".pretty"],
                };
                extensions
                    .iter()
                    .find_map(|extension| name.strip_suffix(extension))
                    .filter(|nickname| !nickname.is_empty())
                    .map(str::to_owned)
            }
        }
    }

    /// The repo folders to look in for libraries of `kind`, in name order:
    /// what a reader lists, then keeps the entries [`Self::nickname_of`]
    /// names. Blank is the repo's top folder.
    pub fn library_folders(&self, kind: TableKind) -> Vec<String> {
        let mut folders: Vec<String> = match self.libraries(kind) {
            LibraryList::Listed { libraries } => libraries
                .iter()
                .map(|library| parent(&library.path).to_owned())
                .collect(),
            LibraryList::EveryLibraryIn { folder } => vec![folder.clone()],
        };
        folders.sort();
        folders.dedup();
        folders
    }

    /// Where the 3D model link `link` (exactly as a footprint has it)
    /// leads in the repo, through the source's path variables.
    ///
    /// The part after the variable must be a plain path: no `..`, no
    /// drive, no empty part right after the variable. A path into a model
    /// folder is matched by the folder's name as written, then the rest
    /// must be plain, the way the import reads our links
    /// (`classify_model_reference`): `${GITPLM_PARTS}/3d-models/a/b.step`
    /// is `3d-models/a/b.step` in a model folder.
    pub fn model_link(&self, link: &str) -> ModelLink {
        for variable in &self.path_variables {
            let Some(folder) = &variable.folder else {
                continue;
            };
            let Some(rest) = link
                .strip_prefix(&format!("${{{}}}", variable.name))
                .and_then(|rest| rest.strip_prefix(['/', '\\']))
            else {
                continue;
            };
            for model_folder in &self.model_folders {
                let Some(lead) = inside(model_folder, folder) else {
                    continue;
                };
                let remainder = if lead.is_empty() {
                    Some(rest)
                } else {
                    // Matched with either slash, as KiCad accepts both.
                    let lead = format!("{lead}/");
                    rest.replace('\\', "/")
                        .starts_with(&lead)
                        .then(|| &rest[lead.len()..])
                };
                if let Some(remainder) = remainder {
                    return match plain_path(remainder) {
                        Some(clean) => ModelLink::InRepo {
                            path: join(model_folder, &clean),
                            in_model_folder: true,
                        },
                        None => ModelLink::Unsafe,
                    };
                }
            }
            return match plain_path(rest) {
                Some(clean) => ModelLink::InRepo {
                    path: join(folder, &clean),
                    in_model_folder: false,
                },
                None => ModelLink::Unsafe,
            };
        }
        ModelLink::Elsewhere
    }

    /// The link a footprint uses for the repo file `path`, such as
    /// `${GITPLM_PARTS}/3d-models/g-SO.3dshapes/SOIC-8.step`: through the
    /// variable whose folder holds it most closely. `None` if no variable
    /// points above it.
    pub fn link_to(&self, path: &str) -> Option<String> {
        self.path_variables
            .iter()
            .filter_map(|variable| {
                let folder = variable.folder.as_deref()?;
                let rest = inside(path, folder)?;
                (!rest.is_empty()).then_some((folder.len(), &variable.name, rest))
            })
            .max_by_key(|(depth, _, _)| *depth)
            .map(|(_, name, rest)| format!("${{{name}}}/{rest}"))
    }

    /// The repo folder where the footprint library `library`'s own 3D
    /// models go, such as `3d-models/g-Crystal.3dshapes`, when the source
    /// has such a rule.
    pub fn own_model_folder(&self, library: &str) -> Option<String> {
        self.models_by_library
            .as_deref()
            .map(|folder| join(folder, &format!("{library}.3dshapes")))
    }
}

/// `folder/name`, or `name` alone for the top folder.
pub fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

/// The folder part of a repo path: blank for a file at the top.
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// The rest of `path` below `folder` (blank when they are the same), or
/// `None` when `path` is not inside it. Blank `folder` is the repo's top.
fn inside<'a>(path: &'a str, folder: &str) -> Option<&'a str> {
    if folder.is_empty() {
        return Some(path);
    }
    let rest = path.strip_prefix(folder)?;
    if rest.is_empty() {
        Some(rest)
    } else {
        rest.strip_prefix('/')
    }
}

/// `relative` as a plain `/`-separated path, or `None` if it could leave
/// its folder: empty, starting with a slash or drive, or holding `..` or
/// `:`. Empty and `.` parts inside are dropped. The same rule as the
/// import's (`safe_relative` in the catalog crate).
fn plain_path(relative: &str) -> Option<String> {
    let bytes = relative.as_bytes();
    if relative.is_empty()
        || relative.starts_with(['/', '\\'])
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        return None;
    }
    let mut parts = Vec::new();
    for part in relative.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            part if part.contains(':') => return None,
            part => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> SourceProfile {
        SourceProfile {
            prefix: "AC".into(),
            symbols: LibraryList::Listed {
                libraries: vec![SourceLibrary {
                    nickname: "Acme".into(),
                    path: "lib/sym/acme_symbols.kicad_sym".into(),
                }],
            },
            footprints: LibraryList::Listed {
                libraries: vec![
                    SourceLibrary {
                        nickname: "Acme_FP".into(),
                        path: "lib/Acme.pretty".into(),
                    },
                    SourceLibrary {
                        nickname: "Top".into(),
                        path: "Top.pretty".into(),
                    },
                ],
            },
            model_folders: vec!["lib/3d".into()],
            path_variables: vec![
                SourceVariable {
                    name: "ACME_LIB".into(),
                    folder: Some("lib".into()),
                },
                SourceVariable {
                    name: "ACME_3D".into(),
                    folder: Some("lib/3d".into()),
                },
                SourceVariable {
                    name: "LOST".into(),
                    folder: None,
                },
            ],
            database_libraries: vec![],
            models_by_library: None,
        }
    }

    #[test]
    fn our_layout_names_libraries_by_their_file_names() {
        let own = SourceProfile::own();
        assert_eq!(
            own.library_path(TableKind::Symbol, "g-res").as_deref(),
            Some("symbols/g-res.kicad_sym")
        );
        assert_eq!(
            own.library_path(TableKind::Footprint, "g-SO").as_deref(),
            Some("footprints/g-SO.pretty")
        );
        // A nickname that could name another folder names nothing.
        for unsafe_name in ["", "..", "a/b", "a\\b", "a:b"] {
            assert_eq!(own.library_path(TableKind::Symbol, unsafe_name), None);
        }
        assert_eq!(
            own.nickname_of(TableKind::Symbol, "symbols/g-res.kicad_sym")
                .as_deref(),
            Some("g-res")
        );
        assert_eq!(
            own.nickname_of(TableKind::Symbol, "symbols/g-new.kicad_symdir")
                .as_deref(),
            Some("g-new")
        );
        assert_eq!(
            own.nickname_of(TableKind::Symbol, "symbols/sub/g-res.kicad_sym"),
            None
        );
        assert_eq!(
            own.nickname_of(TableKind::Footprint, "symbols/g-SO.pretty"),
            None
        );
        assert_eq!(own.library_folders(TableKind::Footprint), ["footprints"]);
        assert!(own.is_own_name("g-res"));
        assert!(!own.is_own_name("gx-res"));
        assert!(!own.is_own_name("Device"));
    }

    #[test]
    fn a_scanned_source_names_libraries_as_listed() {
        let source = client();
        assert_eq!(
            source.library_path(TableKind::Symbol, "Acme").as_deref(),
            Some("lib/sym/acme_symbols.kicad_sym")
        );
        // Exact case, as KiCad matches nicknames.
        assert_eq!(source.library_path(TableKind::Symbol, "acme"), None);
        assert_eq!(
            source
                .nickname_of(TableKind::Footprint, "lib/Acme.pretty")
                .as_deref(),
            Some("Acme_FP")
        );
        assert_eq!(source.library_folders(TableKind::Footprint), ["", "lib"]);
    }

    #[test]
    fn our_model_links_read_as_the_import_reads_them() {
        let own = SourceProfile::own();
        let in_models = |path: &str| ModelLink::InRepo {
            path: path.into(),
            in_model_folder: true,
        };
        assert_eq!(
            own.model_link("${GITPLM_PARTS}/3d-models/g-SO.3dshapes/SOIC-8.step"),
            in_models("3d-models/g-SO.3dshapes/SOIC-8.step")
        );
        assert_eq!(
            own.model_link("${GITPLM_PARTS}\\3d-models\\g-SO.3dshapes\\SOIC-8.step"),
            in_models("3d-models/g-SO.3dshapes/SOIC-8.step")
        );
        assert_eq!(
            own.model_link("${GITPLM_PARTS}/3d-models/./a//b.step"),
            in_models("3d-models/a/b.step")
        );
        // Outside the model folder, or climbing out of it.
        assert_eq!(
            own.model_link("${GITPLM_PARTS}/symbols/x.step"),
            ModelLink::InRepo {
                path: "symbols/x.step".into(),
                in_model_folder: false
            }
        );
        for unsafe_link in [
            "${GITPLM_PARTS}/3d-models/../symbols/x.step",
            "${GITPLM_PARTS}/3d-models//x.step",
            "${GITPLM_PARTS}/3d-models/C:/x.step",
            "${GITPLM_PARTS}/../x.step",
        ] {
            assert_eq!(
                own.model_link(unsafe_link),
                ModelLink::Unsafe,
                "{unsafe_link}"
            );
        }
        for elsewhere in [
            "${GITPLM_PARTSX}/3d-models/x.step",
            "${GITPLM_PARTS}3d-models/x.step",
            "${GITPLM_3DMODELS}/x.step",
            "${KICAD10_3DMODEL_DIR}/R.step",
            "C:/models/x.step",
        ] {
            assert_eq!(
                own.model_link(elsewhere),
                ModelLink::Elsewhere,
                "{elsewhere}"
            );
        }
        assert_eq!(
            own.link_to("3d-models/g-SO.3dshapes/SOIC-8.step")
                .as_deref(),
            Some("${GITPLM_PARTS}/3d-models/g-SO.3dshapes/SOIC-8.step")
        );
        assert_eq!(
            own.own_model_folder("g-SO").as_deref(),
            Some("3d-models/g-SO.3dshapes")
        );
    }

    #[test]
    fn a_client_link_goes_through_the_variable_it_names() {
        let source = client();
        assert_eq!(
            source.model_link("${ACME_3D}/R_0603.step"),
            ModelLink::InRepo {
                path: "lib/3d/R_0603.step".into(),
                in_model_folder: true
            }
        );
        assert_eq!(
            source.model_link("${ACME_LIB}/3d/R_0603.step"),
            ModelLink::InRepo {
                path: "lib/3d/R_0603.step".into(),
                in_model_folder: true
            }
        );
        assert_eq!(
            source.model_link("${ACME_LIB}/other/R.step"),
            ModelLink::InRepo {
                path: "lib/other/R.step".into(),
                in_model_folder: false
            }
        );
        // A variable whose folder is unknown leads nowhere yet.
        assert_eq!(source.model_link("${LOST}/x.step"), ModelLink::Elsewhere);
        // The closest variable names a model.
        assert_eq!(
            source.link_to("lib/3d/R.step").as_deref(),
            Some("${ACME_3D}/R.step")
        );
        assert_eq!(source.link_to("other/R.step"), None);
        assert_eq!(source.own_model_folder("Acme_FP"), None);
    }

    #[test]
    fn a_profile_reads_back_the_same() {
        for source in [SourceProfile::own(), client()] {
            let json = serde_json::to_string(&source).unwrap();
            assert_eq!(
                serde_json::from_str::<SourceProfile>(&json).unwrap(),
                source
            );
        }
    }
}

use super::*;
use std::collections::BTreeMap;

const STEP: &str = "ISO-10303-21;\nHEADER;\nENDSEC;\nEND-ISO-10303-21;\n";

/// A catalog folder, KiCad's 3D models folder and an installed library,
/// each with 3D models.
struct Places {
    dir: tempfile::TempDir,
    places: ModelPlaces,
}

fn places() -> Places {
    let dir = tempfile::tempdir().unwrap();
    let write = |relative: &str, text: &str| {
        let path = dir.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write("catalog/3d-models/g-Package_SO.3dshapes/SOIC-8.step", STEP);
    write("kicad/3dmodels/Resistor_SMD.3dshapes/R_0603.step", STEP);
    write("mine/models/Part.wrl", "#VRML V2.0 utf8\n");
    write(
        "mine/models/Key.step",
        "-----BEGIN OPENSSH PRIVATE KEY-----\n",
    );
    let mut variables = BTreeMap::new();
    variables.insert(
        "MY_LIB_3D".to_owned(),
        dir.path().join("mine").to_string_lossy().into_owned(),
    );
    let places = ModelPlaces {
        catalog_root: dir.path().join("catalog"),
        kicad_models: Some(dir.path().join("kicad").join("3dmodels")),
        variables: PathVariables::new(variables),
    };
    Places { dir, places }
}

impl Places {
    fn path(&self, relative: &str) -> PathBuf {
        let mut path = self.dir.path().to_path_buf();
        path.extend(relative.split('/'));
        path
    }

    fn real(&self, relative: &str) -> PathBuf {
        plain_path(fs::canonicalize(self.path(relative)).unwrap())
    }

    fn resolve(&self, link: &str) -> Resolved {
        resolve(link, false, &self.places, &Supplies::default())
    }
}

#[test]
fn links_resolve_to_our_library_kicads_own_and_installed_libraries() {
    let places = places();
    let soic = places.real("catalog/3d-models/g-Package_SO.3dshapes/SOIC-8.step");
    assert_eq!(
        places.resolve("${GITPLM_PARTS}/3d-models/g-Package_SO.3dshapes/SOIC-8.step"),
        Resolved::Found(ModelSource::Ours, soic.clone())
    );
    // GITPLM_3DMODELS names the same folder.
    assert_eq!(
        places.resolve("${GITPLM_3DMODELS}\\g-Package_SO.3dshapes\\SOIC-8.step"),
        Resolved::Found(ModelSource::Ours, soic)
    );
    // Any KiCad version's variable finds KiCad's own models.
    let resistor = places.real("kicad/3dmodels/Resistor_SMD.3dshapes/R_0603.step");
    for variable in ["KICAD9_3DMODEL_DIR", "KICAD10_3DMODEL_DIR"] {
        let link = ["${", variable, "}/Resistor_SMD.3dshapes/R_0603.step"].concat();
        assert_eq!(
            places.resolve(&link),
            Resolved::Found(ModelSource::Kicad, resistor.clone())
        );
    }
    let part = places.real("mine/models/Part.wrl");
    assert_eq!(
        places.resolve("${MY_LIB_3D}/models/Part.wrl"),
        Resolved::Found(ModelSource::Installed, part.clone())
    );
    // An absolute path is an installed library's too.
    let absolute = places.path("mine/models/Part.wrl");
    assert_eq!(
        places.resolve(&absolute.to_string_lossy()),
        Resolved::Found(ModelSource::Installed, part)
    );
}

#[test]
fn every_reason_a_model_is_not_shown() {
    let places = places();
    let missing = |relative: &str| Resolved::NotShown(NotShown::Missing(places.path(relative)));
    let hidden = resolve(
        "${MY_LIB_3D}/models/Part.wrl",
        true,
        &places.places,
        &Supplies::default(),
    );
    assert_eq!(hidden, Resolved::NotShown(NotShown::Hidden));
    assert_eq!(places.resolve(""), Resolved::NotShown(NotShown::NoLink));
    assert_eq!(
        places.resolve("kicad-embed://inside.step"),
        Resolved::Embedded
    );
    for link in ["${MY_LIB_3D}/models/Part.kicad_mod", "${MY_LIB_3D}/id_rsa"] {
        assert_eq!(
            places.resolve(link),
            Resolved::NotShown(NotShown::NotModelFile),
            "{link}"
        );
    }
    assert_eq!(
        places.resolve("${UNSET_3D}/x.step"),
        Resolved::NotShown(NotShown::VariableNotSet("UNSET_3D".into()))
    );
    assert_eq!(
        places.resolve("x.step"),
        Resolved::NotShown(NotShown::NoFolder)
    );
    // A path from another computer, with a folder but no drive.
    assert!(matches!(
        places.resolve("/home/someone/3d-models/x.step"),
        Resolved::NotShown(NotShown::Missing(_))
    ));
    assert_eq!(
        places.resolve("${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/Gone.step"),
        missing("kicad/3dmodels/Resistor_SMD.3dshapes/Gone.step")
    );
    assert_eq!(
        places.resolve("${GITPLM_PARTS}/3d-models/g-Package_SO.3dshapes/Gone.step"),
        missing("catalog/3d-models/g-Package_SO.3dshapes/Gone.step")
    );
    // A key behind a 3D model's name is never drawn.
    assert_eq!(
        places.resolve("${MY_LIB_3D}/models/Key.step"),
        Resolved::NotShown(NotShown::NotModel(places.path("mine/models/Key.step")))
    );
    // `..` never climbs out of KiCad's folder: the link is read as any
    // other, and KiCad's variable is not one of the person's.
    assert_eq!(
        places.resolve("${KICAD10_3DMODEL_DIR}/../x.step"),
        Resolved::NotShown(NotShown::VariableNotSet("KICAD10_3DMODEL_DIR".into()))
    );
}

#[test]
fn every_reason_has_plain_words_and_says_where_it_looked() {
    let path = PathBuf::from("C:/models/x.step");
    let reasons = [
        NotShown::Hidden,
        NotShown::NoLink,
        NotShown::NotModelFile,
        NotShown::NoFolder,
        NotShown::VariableNotSet("MY_LIB_3D".into()),
        NotShown::Missing(path.clone()),
        NotShown::Linked(path.clone()),
        NotShown::LeadsElsewhere {
            path: path.clone(),
            real: "id_rsa".into(),
        },
        NotShown::NotModel(path.clone()),
        NotShown::Unreadable {
            path: path.clone(),
            why: "access is denied".into(),
        },
        NotShown::HashFailed("the 3D model's bytes do not match their SHA-256".into()),
        NotShown::Unavailable("the team server could not be reached".into()),
        NotShown::ProjectFolder,
        NotShown::NetworkPath,
        NotShown::OutsideDownload,
        NotShown::CliUnreadable(path.clone()),
    ];
    for reason in &reasons {
        let words = reason.words();
        assert!(words.ends_with('.'), "{words}");
        assert!(
            words
                .split(". ")
                .all(|sentence| sentence.split_whitespace().count() <= 30),
            "{words}"
        );
    }
    assert_eq!(
        NotShown::VariableNotSet("MY_LIB_3D".into()).words(),
        "The path variable ${MY_LIB_3D} is not set in KiCad's path settings."
    );
    assert_eq!(NotShown::Missing(path.clone()).path(), Some(path.as_path()));
    assert_eq!(NotShown::Hidden.path(), None);
    assert!(NotShown::Hidden.words().contains("hide flag"));
    assert!(
        NotShown::HashFailed("x".into())
            .words()
            .contains("hash check")
    );
}

#[test]
fn the_catalogs_stored_models_and_chosen_files_come_first() {
    let places = places();
    let stored_copy = places.real("mine/models/Part.wrl");
    let link = "${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603.step";
    let failed = NotShown::HashFailed("its record does not match its SHA-256".into());
    let supplies = Supplies {
        stored: vec![
            Stored {
                link: link.to_owned(),
                key: Some("3d-models/g-Resistor_SMD.3dshapes/R_0603.step".into()),
                copy: Ok(stored_copy.clone()),
            },
            Stored {
                link: "${MY_LIB_3D}/models/Part.wrl".to_owned(),
                key: None,
                copy: Err(failed.clone()),
            },
        ],
        chosen: vec![
            Chosen {
                link: None,
                key: Some("3d-models/g-Package_SO.3dshapes/SOIC-8.step".into()),
                file_name: "chosen.step".into(),
                copy: Ok(PathBuf::from("/cache/a.step")),
            },
            Chosen {
                link: Some("${ELSEWHERE}/b.step".into()),
                key: None,
                file_name: "b-renamed.step".into(),
                copy: Ok(PathBuf::from("/cache/b.step")),
            },
            Chosen {
                link: None,
                key: None,
                file_name: "LM317.STEP".into(),
                copy: Err(NotShown::NotModel(PathBuf::from("LM317.STEP"))),
            },
        ],
        only_chosen: false,
    };
    let resolve = |link: &str| resolve(link, false, &places.places, &supplies);
    // The Catalog's stored model, not KiCad's file with the same link.
    assert_eq!(
        resolve(link),
        Resolved::Found(ModelSource::Catalog, stored_copy)
    );
    assert_eq!(
        resolve("${MY_LIB_3D}/models/Part.wrl"),
        Resolved::NotShown(failed)
    );
    // A chosen file, by its Catalog place, its link or its name.
    assert_eq!(
        resolve("${GITPLM_PARTS}/3d-models/g-Package_SO.3dshapes/SOIC-8.step"),
        Resolved::Found(ModelSource::Chosen, PathBuf::from("/cache/a.step"))
    );
    assert_eq!(
        resolve("${ELSEWHERE}/b.step"),
        Resolved::Found(ModelSource::Chosen, PathBuf::from("/cache/b.step"))
    );
    assert_eq!(
        resolve("${KIPRJMOD}/models/lm317.step"),
        Resolved::NotShown(NotShown::NotModel(PathBuf::from("LM317.STEP")))
    );
}

#[test]
fn a_link_to_a_file_that_is_no_3d_model_leads_nowhere() {
    let places = places();
    let key = places.path("mine/id_rsa");
    fs::write(&key, STEP).unwrap();
    let link = places.path("mine/models/Linked.step");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(&key, &link).is_ok();
    #[cfg(not(windows))]
    let made = std::os::unix::fs::symlink(&key, &link).is_ok();
    if !made {
        eprintln!("links cannot be made here; skipped");
        return;
    }
    assert_eq!(
        places.resolve("${MY_LIB_3D}/models/Linked.step"),
        Resolved::NotShown(NotShown::LeadsElsewhere {
            path: link,
            real: "id_rsa".into()
        })
    );
}

/// A folder junction in our library (Windows makes one without special
/// rights): our library allows no link, even to a real 3D model.
#[cfg(windows)]
#[test]
fn our_library_refuses_a_model_behind_a_junction() {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let places = places();
    let target = places.path("elsewhere");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("SOT-23.step"), STEP).unwrap();
    let junction = places.path("catalog/3d-models/g-Linked.3dshapes");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&target)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .is_ok_and(|output| output.status.success());
    if !made {
        eprintln!("junctions cannot be made here; skipped");
        return;
    }
    assert_eq!(
        places.resolve("${GITPLM_PARTS}/3d-models/g-Linked.3dshapes/SOT-23.step"),
        Resolved::NotShown(NotShown::Linked(junction.join("SOT-23.step")))
    );
}

#[test]
fn only_3d_model_file_names_and_safe_paths_are_read() {
    assert_eq!(
        our_key("${GITPLM_PARTS}/3d-models/g-Diode_SMD.3dshapes/D.stp"),
        Some("3d-models/g-Diode_SMD.3dshapes/D.stp".into())
    );
    for unsafe_link in [
        "${GITPLM_PARTS}/3d-models/../secrets.env",
        "${GITPLM_PARTS}/3d-models/g-X.3dshapes/../../y.step",
        "${GITPLM_PARTS}/other/x.step",
        "${GITPLM_3DMODELS}/a//b.step",
        "${GITPLM_3DMODELS}/C:x.step",
    ] {
        assert_eq!(our_key(unsafe_link), None, "{unsafe_link}");
    }
    // A model our library keeps outside a footprint library's folder.
    assert_eq!(
        our_key("${GITPLM_3DMODELS}/ABS07L-32.768KHZ-T.step"),
        Some("3d-models/ABS07L-32.768KHZ-T.step".into())
    );
    assert_eq!(
        kicad_model("${KICAD8_3DMODEL_DIR}/A.3dshapes/b.wrl", true),
        Some("A.3dshapes/b.wrl")
    );
    for unsafe_link in [
        "${KICAD_3DMODEL_DIR}/A.3dshapes/b.wrl",
        "${KICAD10_3DMODEL_DIR}/../b.wrl",
        // A drive would replace KiCad's folder.
        "${KICAD10_3DMODEL_DIR}/C:/Users/x.step",
        "${KICAD10_3DMODEL_DIR}/A//b.wrl",
    ] {
        assert_eq!(kicad_model(unsafe_link, true), None, "{unsafe_link}");
    }
    // KiCad 5's name, unless the person set it to something else.
    assert_eq!(
        kicad_model("${KISYS3DMOD}/A.3dshapes/b.wrl", true),
        Some("A.3dshapes/b.wrl")
    );
    assert_eq!(kicad_model("${KISYS3DMOD}/A.3dshapes/b.wrl", false), None);
    assert_eq!(
        plain_path(PathBuf::from(r"\\?\C:\a\b.step")),
        PathBuf::from(r"C:\a\b.step")
    );
    assert_eq!(
        plain_path(PathBuf::from(r"\\?\UNC\server\share\b.step")),
        PathBuf::from(r"\\server\share\b.step")
    );
}

#[test]
fn network_paths_are_opened_only_through_the_persons_path_variables() {
    let mut places = places();
    for link in [
        r"\\host\share\x.step",
        "//host/share/x.step",
        r"\\?\UNC\host\share\x.step",
        r"\\.\pipe\x.step",
        r"/\host\share\x.step",
        r"\??\UNC\host\share\x.step",
        "/??/UNC/host/share/x.step",
        r"\??\C:\x.step",
        "/?/x.step",
    ] {
        assert_eq!(
            places.resolve(link),
            Resolved::NotShown(NotShown::NetworkPath),
            "{link}"
        );
    }
    // A variable whose value hides a network path in the link's middle.
    let mut variables = std::collections::BTreeMap::new();
    variables.insert("SLASH".to_owned(), "/".to_owned());
    variables.insert("EMPTY".to_owned(), String::new());
    // A verbatim local folder: it starts like a network path, but opens no
    // connection.
    let local = format!(r"\\?\{}", places.path("mine").display());
    variables.insert("SHARE".to_owned(), local);
    places.places.variables = PathVariables::new(variables);
    // Variables whose values are ordinary, or empty, never make a network
    // path ordinary.
    for link in [
        "/${SLASH}host/share/x.step",
        "${SLASH}/host/share/x.step",
        r"${EMPTY}\\host\share\x.step",
        "${EMPTY}//host/share/x.step",
    ] {
        assert_eq!(
            places.resolve(link),
            Resolved::NotShown(NotShown::NetworkPath),
            "{link}"
        );
    }
    assert!(
        ordinary_path(r"C:\x.step") && ordinary_path("c:/x.step") && ordinary_path("/home/x.step")
    );
    assert!(!ordinary_path("C:x.step") && !ordinary_path(r"\??\C:\x.step"));
    assert!(ordinary_path("/") && ordinary_path(r"\") && !ordinary_path("//"));
    // A folder the person set as a path variable is looked in, even one
    // written like a network path.
    assert!(matches!(
        places.resolve(r"${SHARE}\models\Part.wrl"),
        Resolved::Found(ModelSource::Installed, _)
    ));
}

#[test]
fn a_downloads_footprint_draws_only_its_own_model() {
    let places = places();
    let supplies = Supplies {
        stored: Vec::new(),
        chosen: vec![Chosen {
            link: Some("C:/Users/maker/LM317.step".into()),
            key: None,
            file_name: "LM317.step".into(),
            copy: Ok(PathBuf::from("/cache/lm317.step")),
        }],
        only_chosen: true,
    };
    let resolve = |link: &str| resolve(link, false, &places.places, &supplies);
    assert_eq!(
        resolve("C:/Users/maker/LM317.step"),
        Resolved::Found(ModelSource::Chosen, PathBuf::from("/cache/lm317.step"))
    );
    // The maker's other links, even to a file on this computer, are not
    // looked for.
    let here = places.path("mine/models/Part.wrl");
    for link in [
        here.to_string_lossy().into_owned(),
        "${MY_LIB_3D}/models/Part.wrl".to_owned(),
        r"\\host\share\x.step".to_owned(),
        "${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603.step".to_owned(),
    ] {
        assert_eq!(
            resolve(&link),
            Resolved::NotShown(NotShown::OutsideDownload),
            "{link}"
        );
    }
    assert_eq!(resolve("kicad-embed://inside.step"), Resolved::Embedded);
}

#[test]
fn project_and_kicad_5_links_say_what_they_mean() {
    let places = places();
    assert_eq!(
        places.resolve("${KIPRJMOD}/models/x.step"),
        Resolved::NotShown(NotShown::ProjectFolder)
    );
    assert_eq!(
        places.resolve("${KISYS3DMOD}/Resistor_SMD.3dshapes/R_0603.step"),
        Resolved::Found(
            ModelSource::Kicad,
            places.real("kicad/3dmodels/Resistor_SMD.3dshapes/R_0603.step")
        )
    );
    let words = NotShown::NoFolder.words();
    assert_eq!(
        words,
        "The link has no folder or path variable, so the app can't tell where the file is."
    );
}

#[test]
fn kicad_cli_complaints_name_the_file_they_are_about() {
    let messages = "**** ERR StepFile : Undefined Parsing ****\nreadSTEP() failed on filename 'C:\\m\\Bad.step'.\nNo model for filename 'C:\\m\\Bad.step'.\nBinary GLTF file 'b.glb' created.";
    assert!(cli_could_not_read(messages, Path::new("C:/m/bad.step")));
    assert!(!cli_could_not_read(messages, Path::new("C:/m/good.step")));
    assert!(!cli_could_not_read(
        "Binary GLTF file 'C:/m/bad.step'",
        Path::new("C:/m/bad.step")
    ));
}

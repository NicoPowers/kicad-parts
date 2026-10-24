//! Library layouts end to end: small client repos laid out as real ones
//! are, as bare repos in folders, linked, scanned, reviewed, followed to
//! newer commits and browsed offline.

use super::*;
use desktop_contract::{LayoutChoices, LayoutDoubt, LayoutState};
use kicad_integration::lib_table::TableKind;

/// A footprint linking `links` as its 3D models.
fn footprint(name: &str, links: &[&str]) -> String {
    let models = links
        .iter()
        .map(|link| format!("\t(model \"{link}\")\n"))
        .collect::<Vec<_>>()
        .concat();
    format!("(footprint \"{name}\"\n\t(version 20241229)\n{models})\n")
}

impl Fixture {
    /// A bare repo, as on a git host, whose first commit holds `files`.
    fn client_repo(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let bare = self.path(name);
        fs::create_dir_all(&bare).unwrap();
        self.git(&bare, &["init", "-q", "--bare"]);
        let work = self.path(&format!("{name}-work"));
        fs::create_dir_all(&work).unwrap();
        self.git(&work, &["init", "-q"]);
        self.commit_files(&work, &bare, files, "First libraries");
        bare
    }

    /// Commits `files` in the work folder `work` and pushes to `bare`.
    fn commit_files(&self, work: &Path, bare: &Path, files: &[(&str, &str)], message: &str) {
        for (path, text) in files {
            let file = work.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        }
        self.git(work, &["add", "."]);
        self.git(work, &["commit", "-q", "-m", message]);
        let bare_text = bare.display().to_string();
        self.git(work, &["push", "-q", &bare_text, "HEAD:main"]);
    }

    fn layout_state(&self, repos: &LibraryRepos, team: &str) -> Option<LayoutState> {
        self.status(repos, team).and_then(|status| status.layout)
    }
}

/// Acme keeps lib tables in a `kicad/` folder, with `${KIPRJMOD}`, and its
/// 3D models in a folder of their own behind `${ACME_3D}`.
fn acme_files() -> Vec<(&'static str, String)> {
    vec![
        (
            "kicad/sym-lib-table",
            r#"(sym_lib_table (version 7)
  (lib (name "ACME")(type "KiCad")(uri "${KIPRJMOD}/../lib/acme_symbols.kicad_sym")(options "")(descr "")))"#
                .to_owned(),
        ),
        (
            "kicad/fp-lib-table",
            r#"(fp_lib_table (version 7)
  (lib (name "Acme_FP")(type "KiCad")(uri "${KIPRJMOD}/../lib/Acme.pretty")(options "")(descr "")))"#
                .to_owned(),
        ),
        ("lib/acme_symbols.kicad_sym", SYMBOLS.to_owned()),
        (
            "lib/Acme.pretty/R_0603.kicad_mod",
            footprint("R_0603", &["${ACME_3D}/R_0603.step"]),
        ),
        (
            "lib/Acme.pretty/C_0603.kicad_mod",
            footprint("C_0603", &["${ACME_3D}/c_0603.step"]),
        ),
        ("models/R_0603.step", "ISO-10303-21;\n".to_owned()),
        ("models/C_0603.step", "ISO-10303-21;\n".to_owned()),
    ]
}

/// Beta has no lib tables: models sit next to their footprints, behind
/// `${BETA_LIB}`, and a database library sits at the top.
fn beta_files() -> Vec<(&'static str, String)> {
    vec![
        ("Beta.kicad_sym", SYMBOLS.to_owned()),
        ("beta.kicad_dbl", "{}".to_owned()),
        (
            "Beta.pretty/SOT-23.kicad_mod",
            footprint(
                "SOT-23",
                &[
                    "${BETA_LIB}/Beta.pretty/SOT-23.step",
                    "${BETA_LIB}/Beta.pretty/Gone.step",
                ],
            ),
        ),
        ("Beta.pretty/SOT-23.step", "ISO-10303-21;\n".to_owned()),
    ]
}

fn borrowed<'a>(files: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    files
        .iter()
        .map(|(path, text)| (*path, text.as_str()))
        .collect()
}

#[test]
fn a_linked_repo_is_scanned_reviewed_and_browsed_offline() {
    let fixture = Fixture::new();
    let acme = fixture.client_repo("acme-lib.git", &borrowed(&acme_files()));
    let repos = fixture.repos();
    repos.link(ACME, &acme.display().to_string()).unwrap();
    repos.wait_until_idle();
    assert_eq!(
        fixture.layout_state(&repos, ACME),
        Some(LayoutState::NotReviewed)
    );

    let review = repos.review_layout(ACME, false).unwrap();
    assert_eq!(review.repo_name, "acme-lib");
    assert!(!review.reviewed_before);
    // The team's code is the in-app prefix to start with.
    assert_eq!(review.choices.prefix, "AC");
    let layout = &review.layout;
    let nicknames = |libraries: &[desktop_contract::LayoutLibrary]| -> Vec<String> {
        libraries
            .iter()
            .map(|library| library.nickname.clone())
            .collect()
    };
    // Nicknames as the repo's lib tables give them, never renamed.
    assert_eq!(nicknames(&layout.symbol_libraries), ["ACME"]);
    assert_eq!(nicknames(&layout.footprint_libraries), ["Acme_FP"]);
    assert_eq!(layout.model_folders, ["models"]);
    assert_eq!(layout.path_variables.len(), 1);
    assert_eq!(layout.path_variables[0].folder.as_deref(), Some("models"));
    assert_eq!(
        layout.doubts,
        [LayoutDoubt::CaseDiffers {
            footprint: "lib/Acme.pretty/C_0603.kicad_mod".into(),
            link: "${ACME_3D}/c_0603.step".into(),
            file: "models/C_0603.step".into(),
        }]
    );

    // A choice that doesn't fit the scan is refused plainly.
    let commit = layout.commit.clone();
    let refused = repos.use_layout(&LayoutUseRequest {
        server_url: ACME.into(),
        commit: commit.clone(),
        choices: LayoutChoices {
            prefix: String::new(),
            ..review.choices.clone()
        },
    });
    assert!(refused.unwrap_err().contains("short name"));
    repos
        .use_layout(&LayoutUseRequest {
            server_url: ACME.into(),
            commit,
            choices: LayoutChoices {
                prefix: "ACME".into(),
                ..review.choices.clone()
            },
        })
        .unwrap();
    assert_eq!(
        fixture.layout_state(&repos, ACME),
        Some(LayoutState::Reviewed {
            symbol_libraries: 1,
            footprint_libraries: 1
        })
    );
    // A review again starts from the choices made.
    let again = repos.review_layout(ACME, false).unwrap();
    assert!(again.reviewed_before);
    assert_eq!(again.choices.prefix, "ACME");

    // Browsing works offline, from the app's copy.
    fs::rename(&acme, fixture.path("acme-moved.git")).unwrap();
    let (libraries, notes) = repos.team_libraries();
    assert!(notes.is_empty(), "{notes:?}");
    let [team] = libraries.as_slice() else {
        panic!("one team's libraries: {libraries:?}");
    };
    assert_eq!((team.code.as_str(), team.colour.as_str()), ("AC", "blue"));
    assert_eq!(team.profile.prefix, "ACME");
    let symbols = team
        .profile
        .library_path(TableKind::Symbol, "ACME")
        .unwrap();
    assert_eq!(
        fs::read_to_string(team.folder.join(&symbols)).unwrap(),
        SYMBOLS
    );
    let footprints = team
        .profile
        .library_path(TableKind::Footprint, "Acme_FP")
        .unwrap();
    assert!(
        team.folder
            .join(&footprints)
            .join("R_0603.kicad_mod")
            .is_file()
    );
    // 3D models stay in the copy.
    assert!(!team.folder.join("models").exists());
}

#[test]
fn a_newer_commit_asks_for_a_review_only_when_the_layout_changed() {
    let fixture = Fixture::new();
    let beta = fixture.client_repo("beta-lib.git", &borrowed(&beta_files()));
    let work = fixture.path("beta-lib.git-work");
    let repos = fixture.repos();
    repos.link(BETA, &beta.display().to_string()).unwrap();
    repos.wait_until_idle();
    let review = repos.review_layout(BETA, false).unwrap();
    assert_eq!(review.choices.prefix, "BE");
    assert_eq!(review.layout.database_libraries, ["beta.kicad_dbl"]);
    assert_eq!(
        review.layout.path_variables[0].folder.as_deref(),
        Some(""),
        "BETA_LIB points to the repo's top folder"
    );
    assert!(review.layout.doubts.contains(&LayoutDoubt::MissingModel {
        footprint: "Beta.pretty/SOT-23.kicad_mod".into(),
        link: "${BETA_LIB}/Beta.pretty/Gone.step".into(),
    }));
    repos
        .use_layout(&LayoutUseRequest {
            server_url: BETA.into(),
            commit: review.layout.commit.clone(),
            choices: review.choices.clone(),
        })
        .unwrap();

    // Another 3D model: the same layout, so it moves on silently.
    fixture.commit_files(
        &work,
        &beta,
        &[("Beta.pretty/Spare.step", "ISO")],
        "A model",
    );
    repos.check_again(BETA);
    repos.wait_until_idle();
    assert!(matches!(
        fixture.layout_state(&repos, BETA),
        Some(LayoutState::Reviewed { .. })
    ));
    let moved = repos.review_layout(BETA, false).unwrap();
    assert_ne!(moved.layout.commit, review.layout.commit);

    // A new footprint library: the person should review again.
    let new_footprint = footprint("QFN", &[]);
    fixture.commit_files(
        &work,
        &beta,
        &[("More.pretty/QFN.kicad_mod", new_footprint.as_str())],
        "A library",
    );
    repos.check_again(BETA);
    repos.wait_until_idle();
    assert_eq!(
        fixture.layout_state(&repos, BETA),
        Some(LayoutState::Changed)
    );
    // The layout reviewed before stays in use meanwhile.
    let (libraries, _) = repos.team_libraries();
    assert_eq!(
        libraries[0]
            .profile
            .library_path(TableKind::Footprint, "More"),
        None
    );
    // Scanning again with a fetch, then using it, clears the question.
    let review = repos.review_layout(BETA, true).unwrap();
    assert_eq!(review.layout.footprint_libraries.len(), 2);
    repos
        .use_layout(&LayoutUseRequest {
            server_url: BETA.into(),
            commit: review.layout.commit.clone(),
            choices: review.choices,
        })
        .unwrap();
    assert_eq!(
        fixture.layout_state(&repos, BETA),
        Some(LayoutState::Reviewed {
            symbol_libraries: 1,
            footprint_libraries: 2
        })
    );
}

#[test]
fn removing_the_repo_forgets_its_layout() {
    let fixture = Fixture::new();
    let beta = fixture.client_repo("beta-lib.git", &borrowed(&beta_files()));
    let repos = fixture.repos();
    repos.link(BETA, &beta.display().to_string()).unwrap();
    repos.wait_until_idle();
    let review = repos.review_layout(BETA, false).unwrap();
    repos
        .use_layout(&LayoutUseRequest {
            server_url: BETA.into(),
            commit: review.layout.commit,
            choices: review.choices,
        })
        .unwrap();
    let _ = repos.team_libraries();
    let layouts = fixture.path("library-layouts");
    assert_eq!(
        fs::read_dir(&layouts).unwrap().count(),
        2,
        "a file and its library files"
    );
    repos.unlink(BETA).unwrap();
    assert_eq!(fs::read_dir(&layouts).unwrap().count(), 0);
    // With no repo, there is nothing to review.
    assert!(repos.review_layout(BETA, false).is_err());
}

/// Gamma copies KiCad's own layout: `symbols/`, `footprints/` and one
/// `<library>.3dshapes` folder per footprint library under `3dmodels/`,
/// behind a variable that points to the model folder.
#[test]
fn a_repo_laid_out_like_kicads_own_libraries_is_read_too() {
    let fixture = Fixture::new();
    let soic = footprint(
        "SOIC-8",
        &["${GAMMA_3DMODEL_DIR}/Gamma_SO.3dshapes/SOIC-8.step"],
    );
    let qfn = footprint(
        "QFN-16",
        &["${GAMMA_3DMODEL_DIR}/Gamma_QFN.3dshapes/QFN-16.step"],
    );
    let gamma = fixture.client_repo(
        "gamma-lib.git",
        &[
            ("symbols/Gamma_IC.kicad_sym", SYMBOLS),
            ("symbols/Gamma_Passive.kicad_sym", SYMBOLS),
            ("footprints/Gamma_SO.pretty/SOIC-8.kicad_mod", &soic),
            ("footprints/Gamma_QFN.pretty/QFN-16.kicad_mod", &qfn),
            ("3dmodels/Gamma_SO.3dshapes/SOIC-8.step", "ISO"),
            ("3dmodels/Gamma_QFN.3dshapes/QFN-16.step", "ISO"),
        ],
    );
    let repos = fixture.repos();
    repos.link(ACME, &gamma.display().to_string()).unwrap();
    repos.wait_until_idle();
    let layout = repos.review_layout(ACME, false).unwrap().layout;
    let nicknames: Vec<&str> = layout
        .symbol_libraries
        .iter()
        .chain(&layout.footprint_libraries)
        .map(|library| library.nickname.as_str())
        .collect();
    assert_eq!(
        nicknames,
        ["Gamma_IC", "Gamma_Passive", "Gamma_QFN", "Gamma_SO"]
    );
    assert_eq!(layout.model_folders, ["3dmodels"]);
    assert_eq!(layout.model_files, 2);
    assert_eq!(layout.path_variables.len(), 1);
    assert_eq!(layout.path_variables[0].folder.as_deref(), Some("3dmodels"));
    assert!(layout.doubts.is_empty(), "{:?}", layout.doubts);
}

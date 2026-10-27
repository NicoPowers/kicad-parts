//! Footprints' 3D views (kicad-parts-c9c1), exported by the installed
//! KiCad 10's kicad-cli. Each test skips, and says so, when KiCad is not
//! installed. kicad-cli keeps its settings in the test's own cache folder.
//! Our own libraries are the sample library's (fixtures/sample-library).

use catalog_browser::team::TeamCatalog;
use catalog_browser::{BrowserConfig, CatalogBrowser};
use desktop_contract::{
    FileKind, FilePreviewRequest, Footprint3dRequest, Footprint3dView, KicadStatus, ModelSource,
    PreviewKind, PreviewOutcome, TeamFile,
};
use kicad_integration::SearchLocations;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The sample library, read as our own libraries.
fn sample_library() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/sample-library")
}

/// The browser, and a team catalog nobody is signed in to; `None`, after
/// saying why, when KiCad 10 is not installed.
fn open(dir: &Path) -> Option<(CatalogBrowser, TeamCatalog)> {
    let browser = CatalogBrowser::open(BrowserConfig {
        catalog_root: sample_library(),
        cache_dir: dir.join("cache"),
        settings_file: dir.join("config").join("settings.json"),
        kicad_locations: SearchLocations::Standard,
        // An empty KiCad settings folder: no path variables of the person's.
        kicad_config_dir: Some(dir.join("kicad-settings")),
        kicad_documents_dir: None,
    });
    if !matches!(browser.status().kicad, KicadStatus::Ready { .. }) {
        eprintln!("KiCad 10 is not installed, so the 3D export tests are skipped.");
        return None;
    }
    let team = TeamCatalog::new();
    Some((browser, team))
}

/// The drawing key of the footprint `reference`, as a close-up has it.
fn drawing_key(browser: &CatalogBrowser, reference: &str) -> String {
    match browser.preview_field("ANY", PreviewKind::Footprint, reference) {
        PreviewOutcome::Ready(set) => set.images[0].drawing_key.clone().expect("a drawing key"),
        PreviewOutcome::Problem(problem) => panic!("{reference}: {problem:?}"),
    }
}

/// The drawing key of a footprint file chosen in Edit part.
fn file_key(browser: &CatalogBrowser, name: &str, footprint: &str) -> String {
    use base64::Engine;
    let request = FilePreviewRequest {
        file: TeamFile {
            kind: FileKind::Footprint,
            file_name: format!("{name}.kicad_mod"),
            base64: base64::engine::general_purpose::STANDARD.encode(footprint),
            symbol: None,
            model_key: None,
            replaces: None,
            label: None,
            installed: None,
            no_models: false,
            choice: None,
        },
        reference: format!("g-Test:{name}"),
        selection: None,
    };
    match browser.preview_file(&request) {
        PreviewOutcome::Ready(set) => set.images[0].drawing_key.clone().unwrap(),
        PreviewOutcome::Problem(problem) => panic!("{problem:?}"),
    }
}

/// The JSON chunk of a GLB, once the GLB is shown to be whole.
fn glb_json(bytes: &[u8]) -> serde_json::Value {
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    assert_eq!(&bytes[..4], b"glTF");
    assert_eq!(word(4), 2, "glTF version 2");
    assert_eq!(word(8), bytes.len(), "the GLB's length is its own");
    let json_length = word(12);
    assert_eq!(&bytes[16..20], b"JSON");
    // The binary chunk with the meshes' data follows.
    assert_eq!(&bytes[24 + json_length..28 + json_length], b"BIN\0");
    serde_json::from_slice(&bytes[20..20 + json_length]).unwrap()
}

fn mesh_names(json: &serde_json::Value) -> Vec<String> {
    json["meshes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|mesh| mesh["name"].as_str().map(str::to_owned))
        .collect()
}

fn made(browser: &CatalogBrowser, team: &TeamCatalog, key: &str) -> (Footprint3dView, f64) {
    let started = Instant::now();
    let view = browser.footprint_3d(
        &Footprint3dRequest {
            key: key.to_owned(),
            ..Footprint3dRequest::default()
        },
        team,
    );
    (view, started.elapsed().as_secs_f64())
}

#[test]
fn our_r_0603_is_exported_with_kicads_own_model() {
    let dir = tempfile::tempdir().unwrap();
    let Some((browser, team)) = open(dir.path()) else {
        return;
    };
    let key = drawing_key(&browser, "g-Resistor_SMD:R_0603_1608Metric");
    let (view, first) = made(&browser, &team, &key);
    assert_eq!(view.problem, None, "{view:?}");
    let [model] = view.models.as_slice() else {
        panic!("one model: {view:?}");
    };
    assert_eq!(
        model.link,
        "${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step"
    );
    assert_eq!(model.source, Some(ModelSource::Kicad));
    let path = PathBuf::from(model.path.as_deref().unwrap());
    assert!(path.is_absolute() && path.is_file(), "{path:?}");
    assert!(path.ends_with("3dmodels/Resistor_SMD.3dshapes/R_0603_1608Metric.step"));
    assert!(view.notes.is_empty(), "{:?}", view.notes);

    let glb = view.glb.expect("a 3D view");
    assert!(!glb.from_cache);
    let bytes = browser.footprint_glb(&glb.key).expect("the GLB");
    assert_eq!(bytes.len() as u64, glb.size);
    let names = mesh_names(&glb_json(&bytes));
    // The model's own mesh, named by its STEP file, and the board's pads.
    assert!(
        names.iter().any(|name| name == "R_0603_1608Metric"),
        "{names:?}"
    );
    assert!(names.iter().any(|name| name.ends_with("_pad")), "{names:?}");
    assert!(names.iter().any(|name| name.ends_with("_PCB")), "{names:?}");

    // The second time is served from the cache.
    let (again, second) = made(&browser, &team, &key);
    let again = again.glb.expect("a 3D view");
    assert!(again.from_cache && again.key == glb.key);
    eprintln!("R_0603 3D view: made in {first:.2} s, from the cache in {second:.3} s");
    assert!(browser.footprint_glb("v2/../../secrets.env").is_none());
}

#[test]
fn our_own_step_and_vrml_models_are_shown() {
    let dir = tempfile::tempdir().unwrap();
    let Some((browser, team)) = open(dir.path()) else {
        return;
    };
    for (reference, link, mesh) in [
        (
            "g-Capacitor_SMD:C_0603_1608Metric",
            "${GITPLM_PARTS}/3d-models/g-Capacitor_SMD.3dshapes/C_0603_1608Metric.step",
            "C_0603_1608Metric",
        ),
        (
            "g-Package_TO_SOT_SMD:SOT-23-5",
            "${GITPLM_PARTS}/3d-models/g-Package_TO_SOT_SMD.3dshapes/SOT-23-5.wrl",
            "SOT-23-5",
        ),
    ] {
        let key = drawing_key(&browser, reference);
        let (view, _) = made(&browser, &team, &key);
        assert_eq!(view.problem, None, "{view:?}");
        let [model] = view.models.as_slice() else {
            panic!("one model: {view:?}");
        };
        assert_eq!(model.link, link);
        assert_eq!(model.source, Some(ModelSource::Ours), "{reference}");
        assert_eq!(model.problem, None, "{reference}");
        let path = PathBuf::from(model.path.as_deref().unwrap());
        assert!(path.is_file(), "{path:?}");
        let bytes = browser
            .footprint_glb(&view.glb.expect("a 3D view").key)
            .expect("the GLB");
        let names = mesh_names(&glb_json(&bytes));
        assert!(
            names.iter().any(|name| name.starts_with(mesh)),
            "{reference}: {names:?}"
        );
    }
}

#[test]
fn a_big_connector_is_exported() {
    let dir = tempfile::tempdir().unwrap();
    let Some((browser, team)) = open(dir.path()) else {
        return;
    };
    let reference = "Connector_Samtec:Samtec_FMC_ASP-134602-01_10x40_P1.27mm_Vertical";
    let key = drawing_key(&browser, reference);
    let (view, seconds) = made(&browser, &team, &key);
    assert_eq!(view.problem, None, "{view:?}");
    assert_eq!(view.models[0].source, Some(ModelSource::Kicad));
    let glb = view.glb.expect("a 3D view");
    eprintln!(
        "{reference} 3D view: {} MB, made in {seconds:.2} s",
        glb.size / (1024 * 1024)
    );
    let bytes = browser.footprint_glb(&glb.key).unwrap();
    assert!(mesh_names(&glb_json(&bytes)).len() > 3);
}

#[test]
fn a_footprint_whose_models_cannot_be_shown_still_shows_its_board() {
    let dir = tempfile::tempdir().unwrap();
    let Some((browser, team)) = open(dir.path()) else {
        return;
    };
    let footprint = "(footprint \"No_Models\" (version 20241229) (layer \"F.Cu\") (attr smd)
  (fp_rect (start -1.5 -1) (end 1.5 1) (stroke (width 0.05) (type solid)) (fill no) (layer \"F.CrtYd\"))
  (fp_line (start -1 -0.8) (end 1 -0.8) (stroke (width 0.12) (type solid)) (layer \"F.SilkS\"))
  (pad \"1\" smd rect (at -0.8 0) (size 0.8 1) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))
  (pad \"2\" smd rect (at 0.8 0) (size 0.8 1) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))
  (model \"${KICAD10_3DMODEL_DIR}/Nothing.3dshapes/Missing.step\")
  (model \"${MY_LIB_3D}/Part.step\")
  (model \"${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step\" (hide yes))
)
";
    let key = file_key(&browser, "No_Models", footprint);
    let (view, _) = made(&browser, &team, &key);
    assert_eq!(view.problem, None, "{view:?}");
    let problems: Vec<&str> = view
        .models
        .iter()
        .map(|model| model.problem.as_deref().unwrap_or("shown"))
        .collect();
    assert_eq!(
        problems,
        [
            "The file is missing.",
            "The path variable ${MY_LIB_3D} is not set in KiCad's path settings.",
            "The footprint hides this model with KiCad's hide flag, so it is not drawn, as in KiCad.",
        ]
    );
    // Where the missing file was looked for.
    let tried = view.models[0].path.as_deref().unwrap();
    assert!(
        tried.ends_with("Missing.step") && Path::new(tried).is_absolute(),
        "{tried}"
    );
    assert_eq!(
        view.notes,
        [
            "No 3D model can be shown, so only the board with its pads, silkscreen and mask is shown."
        ]
    );
    let bytes = browser.footprint_glb(&view.glb.unwrap().key).unwrap();
    let names = mesh_names(&glb_json(&bytes));
    assert!(
        names.iter().all(|name| name.starts_with("board_")),
        "{names:?}"
    );
    assert!(names.iter().any(|name| name == "board_pad"), "{names:?}");
}

/// kicad-cli 10 ends with code 2 when it cannot read a STEP file, but still
/// writes the board. The view is made, the model is marked as not drawn,
/// and the result is cached like any other.
#[test]
fn a_model_kicad_cli_cannot_read_is_marked_and_the_board_still_shown() {
    let dir = tempfile::tempdir().unwrap();
    let Some((browser, team)) = open(dir.path()) else {
        return;
    };
    let truncated = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Truncated.step");
    let link = truncated.to_string_lossy().replace('\\', "/");
    let footprint = format!(
        "(footprint \"Truncated_Model\" (version 20241229) (layer \"F.Cu\") (attr smd)
  (fp_rect (start -1.5 -1) (end 1.5 1) (stroke (width 0.05) (type solid)) (fill no) (layer \"F.CrtYd\"))
  (pad \"1\" smd rect (at -0.8 0) (size 0.8 1) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))
  (pad \"2\" smd rect (at 0.8 0) (size 0.8 1) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))
  (model \"{link}\")
)
"
    );
    let key = file_key(&browser, "Truncated_Model", &footprint);
    for time in ["first", "from the cache"] {
        let (view, _) = made(&browser, &team, &key);
        assert_eq!(view.problem, None, "{time}: {view:?}");
        let [model] = view.models.as_slice() else {
            panic!("{time}: one model: {view:?}");
        };
        assert_eq!(model.source, None, "{time}");
        assert_eq!(
            model.problem.as_deref(),
            Some("kicad-cli could not read this file."),
            "{time}"
        );
        assert!(
            model
                .path
                .as_deref()
                .is_some_and(|path| path.ends_with("Truncated.step"))
        );
        assert_eq!(
            view.notes[0],
            "No 3D model can be shown, so only the board with its pads, silkscreen and mask is shown.",
            "{time}"
        );
        assert!(
            view.notes[1].contains("readSTEP() failed"),
            "{time}: {:?}",
            view.notes
        );
        let glb = view.glb.expect("the board is still shown");
        assert_eq!(glb.from_cache, time != "first");
        let bytes = browser.footprint_glb(&glb.key).unwrap();
        assert!(
            mesh_names(&glb_json(&bytes))
                .iter()
                .all(|name| name.starts_with("board_"))
        );
    }
}

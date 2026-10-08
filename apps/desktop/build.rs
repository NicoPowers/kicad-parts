use std::fs;
use std::path::Path;

fn main() {
    // `tauri::generate_context!` embeds the built Leptos UI and refuses to
    // compile when the directory is missing. A fresh clone has not run
    // `trunk build` yet, so give `cargo check`, clippy and tests a
    // placeholder page that says how to build the real UI.
    let dist = Path::new("../desktop-ui/dist");
    if !dist.join("index.html").exists() {
        fs::create_dir_all(dist).expect("create UI dist directory");
        fs::write(
            dist.join("index.html"),
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>KiCad Parts</title></head>\
             <body><main><h1>KiCad Parts</h1><p>The UI has not been built. Run \
             <code>trunk build</code> in <code>apps/desktop-ui</code>, then rebuild the desktop app.</p></main></body></html>",
        )
        .expect("write placeholder UI");
    }
    println!("cargo:rerun-if-changed=../desktop-ui/dist");
    tauri_build::build();
}

//! The "KiCad libraries" section of the KiCad settings panel
//! (kicad-parts-f397): whether the libraries made from the team catalog are
//! up to date, Update now and Open folder, and what to add to KiCad once.

use crate::backend;
use desktop_contract::{KicadLibrariesState, KicadLibrariesStatus, OdbcDriver};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::time::Duration;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// The browser's clipboard: `navigator.clipboard.writeText`.
    #[wasm_bindgen(js_namespace = ["navigator", "clipboard"], js_name = writeText, catch)]
    async fn write_text(text: &str) -> Result<JsValue, JsValue>;
}

/// How long "Copied" shows on a Copy button.
const COPIED_FOR: Duration = Duration::from_secs(2);

/// `count` with `singular` or `plural`.
fn counted(count: u32, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

/// A time in milliseconds since 1970 as local hours and minutes, such as
/// `14:02`, in a `<time>` with the full UTC time for machines.
fn local_time(at_ms: u64) -> impl IntoView {
    let date = js_sys::Date::new(&JsValue::from_f64(at_ms as f64));
    let shown = format!("{:02}:{:02}", date.get_hours(), date.get_minutes());
    let datetime: String = date.to_iso_string().into();
    view! { <time datetime=datetime>{shown}</time> }
}

#[component]
pub fn KicadLibrariesSection(status: RwSignal<Option<KicadLibrariesStatus>>) -> impl IntoView {
    let asked = RwSignal::new(false);
    // Which Copy button just copied, or why copying failed.
    let copied = RwSignal::new(None::<&'static str>);
    let copy_failed = RwSignal::new(false);
    let update_now = move |_| {
        asked.set(true);
        spawn_local(async move {
            if let Ok(latest) = backend::kicad_libraries_update().await {
                status.set(Some(latest));
            }
            asked.set(false);
        });
    };
    let open_folder = move |_| {
        spawn_local(async move {
            let _ = backend::kicad_libraries_open_folder().await;
        });
    };
    let copy = move |which: &'static str, text: String| {
        spawn_local(async move {
            let worked = write_text(&text).await.is_ok();
            copy_failed.set(!worked);
            copied.set(worked.then_some(which));
            set_timeout(move || copied.set(None), COPIED_FOR);
        });
    };
    let state_line = move || {
        let Some(known) = status.get() else {
            return view! { <p class="kicad-libraries-status" role="status">"Looking…"</p> }
                .into_any();
        };
        match known.state {
            KicadLibrariesState::SignedOut => view! {
                <p class="kicad-libraries-status" role="status" data-state="signed-out">
                    "Sign in to the team catalog to make the KiCad libraries."
                </p>
            }
            .into_any(),
            KicadLibrariesState::Waiting => view! {
                <p class="kicad-libraries-status" role="status" data-state="waiting">
                    "Waiting for the team catalog…"
                </p>
            }
            .into_any(),
            KicadLibrariesState::Updating => view! {
                <p class="kicad-libraries-status" role="status" data-state="updating">
                    "Updating…"
                </p>
            }
            .into_any(),
            KicadLibrariesState::UpToDate {
                at_ms,
                current,
                older,
                archived,
                restart_kicad,
            } => view! {
                <p
                    class="kicad-libraries-status"
                    role="status"
                    data-state="up-to-date"
                    data-at=at_ms.to_string()
                >
                    "Up to date at "
                    {local_time(at_ms)}
                    {format!(
                        ": {}, {} and {}.",
                        counted(current, "part", "parts"),
                        counted(older, "older revision", "older revisions"),
                        counted(archived, "archived revision", "archived revisions"),
                    )}
                    {restart_kicad.then_some(
                        " The groups or fields changed, so restart KiCad to see them.",
                    )}
                </p>
            }
            .into_any(),
            KicadLibrariesState::Failed { at_ms, reason } => view! {
                <p
                    class="kicad-libraries-status kicad-libraries-failed"
                    role="status"
                    data-state="failed"
                    data-at=at_ms.to_string()
                >
                    "The update at "
                    {local_time(at_ms)}
                    " failed. "
                    {reason}
                </p>
            }
            .into_any(),
        }
    };
    let signed_out = move || {
        status.with(|known| {
            known
                .as_ref()
                .is_none_or(|known| known.state == KicadLibrariesState::SignedOut)
        })
    };
    let problems = move || {
        let problems = status.with(|known| {
            known
                .as_ref()
                .map(|known| known.problems.clone())
                .unwrap_or_default()
        });
        (!problems.is_empty()).then(|| {
            view! {
                <details class="kicad-libraries-problems">
                    <summary>{format!(
                        "{} could not be made",
                        counted(problems.len() as u32, "item", "items")
                    )}</summary>
                    <ul>
                        {problems.into_iter().map(|problem| view! { <li>{problem}</li> }).collect_view()}
                    </ul>
                </details>
            }
        })
    };
    let driver = move || {
        let driver = status.with(|known| known.as_ref().map(|known| known.odbc_driver))?;
        Some(match driver {
            OdbcDriver::Installed => view! {
                <p class="kicad-libraries-driver">
                    "KiCad reads the parts through the SQLite3 ODBC driver, which is installed."
                </p>
            }
            .into_any(),
            OdbcDriver::Missing => view! {
                <p class="kicad-libraries-driver kicad-libraries-missing">
                    "KiCad reads the parts through the SQLite3 ODBC driver, which is not installed. Install the 64-bit SQLite3 ODBC driver, then restart KiCad."
                </p>
            }
            .into_any(),
            OdbcDriver::Unknown => view! {
                <p class="kicad-libraries-driver">
                    "KiCad reads the parts through the SQLite3 ODBC driver. Check that it is installed."
                </p>
            }
            .into_any(),
        })
    };
    let setup = move || {
        let known = status.get()?;
        if known.libraries.is_empty() {
            return None;
        }
        let table = move |which: &'static str, title: &'static str, text: String| {
            let heading = format!("kicad-{which}-libraries-heading");
            let copied_text = text.clone();
            view! {
                <div class="kicad-library-table">
                    <div class="kicad-library-table-head">
                        <h4 id=heading.clone()>{title}</h4>
                        <button
                            type="button"
                            id=format!("kicad-{which}-libraries-copy")
                            on:click=move |_| copy(which, copied_text.clone())
                        >
                            {move || if copied.get() == Some(which) { "Copied" } else { "Copy" }}
                        </button>
                    </div>
                    <pre
                        id=format!("kicad-{which}-libraries")
                        tabindex="0"
                        aria-labelledby=heading
                    >
                        {text}
                    </pre>
                </div>
            }
        };
        Some(view! {
            <h3>"Add them to KiCad once"</h3>
            <ol class="kicad-libraries-steps">
                <li>
                    "Copy the symbol libraries. In KiCad, choose Preferences, then Manage Symbol Libraries. On the Global Libraries tab, right-click the table, choose Paste, then choose OK."
                </li>
                <li>
                    "Copy the footprint libraries, and paste them the same way in Preferences, then Manage Footprint Libraries."
                </li>
                <li>
                    "If KiCad already lists a library with the same nickname, such as one from the CSV catalog, delete that row first."
                </li>
            </ol>
            <p class="kicad-libraries-note">
                {format!(
                    "New parts and revisions reach KiCad within about {} seconds: close and open the Add Symbol window to see them.",
                    known.refresh_seconds
                )}
            </p>
            {move || copy_failed.get().then(|| view! {
                <p class="kicad-libraries-copy-failed">
                    "Copying did not work. Select the text and press Ctrl+C instead."
                </p>
            })}
            {table("symbol", "Symbol libraries", known.symbol_table.clone())}
            {table("footprint", "Footprint libraries", known.footprint_table.clone())}
        })
    };
    view! {
        <section class="kicad-libraries" aria-labelledby="kicad-libraries-heading">
            <h2 id="kicad-libraries-heading">"KiCad libraries"</h2>
            <p>
                "The team's parts, one entry for each revision, so a design never switches to newer CAD by itself. They are in "
                <code class="kicad-libraries-folder">
                    {move || status.with(|known| known.as_ref().map(|known| known.folder.clone()))}
                </code>
                "."
            </p>
            {state_line}
            <div class="override-row">
                <button
                    type="button"
                    id="kicad-libraries-update"
                    disabled=move || signed_out() || asked.get()
                    on:click=update_now
                >
                    "Update now"
                </button>
                <button type="button" id="kicad-libraries-open" on:click=open_folder>
                    "Open folder"
                </button>
            </div>
            {problems}
            {driver}
            {setup}
        </section>
    }
}

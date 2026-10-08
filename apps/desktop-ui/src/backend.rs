//! Calls into the Tauri shell. Payload shapes come from `desktop-contract`.

use desktop_contract::{
    AppStatus, PartDetail, PreviewKind, PreviewOutcome, PreviewRequest, SearchRequest,
    SearchResponse, command,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke, catch)]
    async fn tauri_invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

async fn call<A: Serialize, R: DeserializeOwned>(cmd: &str, args: &A) -> Result<R, String> {
    let serializer = serde_wasm_bindgen::Serializer::json_compatible();
    let args = args
        .serialize(&serializer)
        .map_err(|error| error.to_string())?;
    let value = tauri_invoke(cmd, args).await.map_err(|error| {
        error
            .as_string()
            .unwrap_or_else(|| format!("{cmd} failed: {error:?}"))
    })?;
    serde_wasm_bindgen::from_value(value).map_err(|error| error.to_string())
}

#[derive(Serialize)]
struct NoArgs {}

#[derive(Serialize)]
struct OverrideArgs {
    path: Option<String>,
}

#[derive(Serialize)]
struct SearchArgs {
    request: SearchRequest,
}

#[derive(Serialize)]
struct DetailArgs<'a> {
    ipn: &'a str,
}

#[derive(Serialize)]
struct PreviewArgs {
    request: PreviewRequest,
}

pub async fn app_status() -> Result<AppStatus, String> {
    call(command::APP_STATUS, &NoArgs {}).await
}

pub async fn rediscover_kicad() -> Result<AppStatus, String> {
    call(command::REDISCOVER_KICAD, &NoArgs {}).await
}

pub async fn set_kicad_override(path: Option<String>) -> Result<AppStatus, String> {
    call(command::SET_KICAD_OVERRIDE, &OverrideArgs { path }).await
}

pub async fn search(query: String, limit: usize) -> Result<SearchResponse, String> {
    call(
        command::SEARCH_PARTS,
        &SearchArgs {
            request: SearchRequest { query, limit },
        },
    )
    .await
}

pub async fn part_detail(ipn: &str) -> Result<Option<PartDetail>, String> {
    call(command::PART_DETAIL, &DetailArgs { ipn }).await
}

pub async fn preview(ipn: String, kind: PreviewKind) -> Result<PreviewOutcome, String> {
    call(
        command::PART_PREVIEW,
        &PreviewArgs {
            request: PreviewRequest { ipn, kind },
        },
    )
    .await
}

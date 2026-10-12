//! Calls into the Tauri shell. Payload shapes come from `desktop-contract`.

use desktop_contract::{
    AppStatus, FilePreviewRequest, PartDetail, PreviewOutcome, PreviewRequest, PublishResult,
    SearchRequest, SearchResponse, SignInRequest, TeamArchiveRequest, TeamCsvReviewRequest,
    TeamDownload, TeamDownloadRequest, TeamEditRequest, TeamFileReading, TeamFileRequest,
    TeamIpnCheck, TeamIpnRequest, TeamNewPartRequest, TeamPart, TeamStatus, command,
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

/// Asks for a part's symbol or footprint preview; see [`PreviewRequest`].
pub async fn preview(request: PreviewRequest) -> Result<PreviewOutcome, String> {
    call(command::PART_PREVIEW, &PreviewArgs { request }).await
}

#[derive(Serialize)]
struct SignInArgs {
    request: SignInRequest,
}

#[derive(Serialize)]
struct EditArgs {
    request: TeamEditRequest,
}

#[derive(Serialize)]
struct ArchiveArgs {
    request: TeamArchiveRequest,
}

#[derive(Serialize)]
struct CsvReviewArgs {
    request: TeamCsvReviewRequest,
}

pub async fn team_status() -> Result<TeamStatus, String> {
    call(command::TEAM_STATUS, &NoArgs {}).await
}

pub async fn team_sign_in(request: SignInRequest) -> Result<TeamStatus, String> {
    call(command::TEAM_SIGN_IN, &SignInArgs { request }).await
}

pub async fn team_sign_out() -> Result<TeamStatus, String> {
    call(command::TEAM_SIGN_OUT, &NoArgs {}).await
}

pub async fn team_part(ipn: &str) -> Result<Option<TeamPart>, String> {
    call(command::TEAM_PART, &DetailArgs { ipn }).await
}

pub async fn team_edit(request: TeamEditRequest) -> Result<PublishResult, String> {
    call(command::TEAM_EDIT, &EditArgs { request }).await
}

pub async fn team_archive(request: TeamArchiveRequest) -> Result<PublishResult, String> {
    call(command::TEAM_ARCHIVE, &ArchiveArgs { request }).await
}

pub async fn team_csv_review(request: TeamCsvReviewRequest) -> Result<PublishResult, String> {
    call(command::TEAM_CSV_REVIEW, &CsvReviewArgs { request }).await
}

#[derive(Serialize)]
struct ReadFileArgs {
    request: TeamFileRequest,
}

#[derive(Serialize)]
struct PreviewFileArgs {
    request: FilePreviewRequest,
}

/// Reads a symbol library or footprint file chosen in Edit part.
pub async fn team_read_file(request: TeamFileRequest) -> Result<TeamFileReading, String> {
    call(command::TEAM_READ_FILE, &ReadFileArgs { request }).await
}

/// Draws a chosen symbol or footprint file before it is saved.
pub async fn team_preview_file(request: FilePreviewRequest) -> Result<PreviewOutcome, String> {
    call(command::TEAM_PREVIEW_FILE, &PreviewFileArgs { request }).await
}

#[derive(Serialize)]
struct DownloadArgs {
    request: TeamDownloadRequest,
}

#[derive(Serialize)]
struct IpnArgs {
    request: TeamIpnRequest,
}

#[derive(Serialize)]
struct NewPartArgs {
    request: TeamNewPartRequest,
}

/// Reads the files chosen for Add part.
pub async fn team_read_download(request: TeamDownloadRequest) -> Result<TeamDownload, String> {
    call(command::TEAM_READ_DOWNLOAD, &DownloadArgs { request }).await
}

/// Whether an IPN can be a new part's.
pub async fn team_check_ipn(ipn: String) -> Result<TeamIpnCheck, String> {
    call(
        command::TEAM_CHECK_IPN,
        &IpnArgs {
            request: TeamIpnRequest { ipn },
        },
    )
    .await
}

/// Saves a new part.
pub async fn team_add_part(request: TeamNewPartRequest) -> Result<PublishResult, String> {
    call(command::TEAM_ADD_PART, &NewPartArgs { request }).await
}

//! Calls into the Tauri shell. Payload shapes come from `desktop-contract`.

use desktop_contract::{
    AppStatus, FilePreviewRequest, Footprint3dRequest, Footprint3dView, KicadLibrariesStatus,
    LibrariesView, LibraryCheckView, LibraryItemDetail, LibraryItemRequest, LibraryItems,
    LibraryItemsRequest, OurLibraries, PartDetail, PreviewOutcome, PreviewRequest, PublishResult,
    SearchRequest, SearchResponse, SignInRequest, TeamArchiveRequest, TeamCsvReviewRequest,
    TeamDownload, TeamDownloadRequest, TeamEditRequest, TeamFileReading, TeamFileRequest,
    TeamIpnCheck, TeamIpnRequest, TeamMpnRequest, TeamNewPartRequest, TeamPart, TeamPick,
    TeamPickRequest, TeamSameMpn, TeamsStatus, command,
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

pub async fn search(
    query: String,
    limit: usize,
    selected: Option<String>,
    category: Option<String>,
    uses: Option<String>,
) -> Result<SearchResponse, String> {
    call(
        command::SEARCH_PARTS,
        &SearchArgs {
            request: SearchRequest {
                query,
                limit,
                selected,
                category,
                uses,
            },
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

pub async fn team_status() -> Result<TeamsStatus, String> {
    call(command::TEAM_STATUS, &NoArgs {}).await
}

pub async fn kicad_libraries_status() -> Result<KicadLibrariesStatus, String> {
    call(command::KICAD_LIBRARIES_STATUS, &NoArgs {}).await
}

pub async fn kicad_libraries_update() -> Result<KicadLibrariesStatus, String> {
    call(command::KICAD_LIBRARIES_UPDATE, &NoArgs {}).await
}

pub async fn kicad_libraries_open_folder() -> Result<(), String> {
    call(command::KICAD_LIBRARIES_OPEN_FOLDER, &NoArgs {}).await
}

#[derive(Serialize)]
struct KindArgs<'a> {
    kind: &'a str,
}

pub async fn kicad_libraries_copied(kind: &str) -> Result<KicadLibrariesStatus, String> {
    call(command::KICAD_LIBRARIES_COPIED, &KindArgs { kind }).await
}

#[derive(Serialize)]
struct CodeArgs<'a> {
    code: &'a str,
}

pub async fn team_code_check(code: &str) -> Result<desktop_contract::TeamCodeCheck, String> {
    call(command::TEAM_CODE_CHECK, &CodeArgs { code }).await
}

pub async fn team_code_set(code: &str) -> Result<KicadLibrariesStatus, String> {
    call(command::TEAM_CODE_SET, &CodeArgs { code }).await
}

pub async fn team_sign_in(request: SignInRequest) -> Result<TeamsStatus, String> {
    call(command::TEAM_SIGN_IN, &SignInArgs { request }).await
}

/// One team, by its server address. Tauri takes argument names in camel
/// case.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerArgs<'a> {
    server_url: &'a str,
}

pub async fn team_sign_out(server_url: &str) -> Result<TeamsStatus, String> {
    call(command::TEAM_SIGN_OUT, &ServerArgs { server_url }).await
}

pub async fn team_remove(server_url: &str) -> Result<TeamsStatus, String> {
    call(command::TEAM_REMOVE, &ServerArgs { server_url }).await
}

pub async fn team_show(server_url: &str) -> Result<TeamsStatus, String> {
    call(command::TEAM_SHOW, &ServerArgs { server_url }).await
}

/// The person said the team at `server_url` moved there, so its KiCad
/// libraries take updates from it.
pub async fn kicad_libraries_use_new_server(server_url: &str) -> Result<TeamsStatus, String> {
    call(
        command::KICAD_LIBRARIES_USE_NEW_SERVER,
        &ServerArgs { server_url },
    )
    .await
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
pub async fn team_check_ipn(ipn: String, prefix: String) -> Result<TeamIpnCheck, String> {
    call(
        command::TEAM_CHECK_IPN,
        &IpnArgs {
            request: TeamIpnRequest { ipn, prefix },
        },
    )
    .await
}

/// Saves a new part.
pub async fn team_add_part(request: TeamNewPartRequest) -> Result<PublishResult, String> {
    call(command::TEAM_ADD_PART, &NewPartArgs { request }).await
}

#[derive(Serialize)]
struct MpnArgs {
    request: TeamMpnRequest,
}

/// The parts that already have an MPN.
pub async fn team_check_mpn(mpn: String, manufacturer: String) -> Result<Vec<TeamSameMpn>, String> {
    call(
        command::TEAM_CHECK_MPN,
        &MpnArgs {
            request: TeamMpnRequest { mpn, manufacturer },
        },
    )
    .await
}

/// What the library check found, for someone who can edit; `None` for
/// anyone else. The app works it out again only when the team catalog
/// changed.
pub async fn team_library_check() -> Result<Option<LibraryCheckView>, String> {
    call(command::TEAM_LIBRARY_CHECK, &NoArgs {}).await
}

#[derive(Serialize)]
struct LibrariesArgs {
    refresh: bool,
}

#[derive(Serialize)]
struct LibraryItemsArgs {
    request: LibraryItemsRequest,
}

#[derive(Serialize)]
struct LibraryItemArgs {
    request: LibraryItemRequest,
}

/// The Libraries browser's libraries and reading progress; `refresh`
/// checks every library again in the background.
pub async fn libraries(refresh: bool) -> Result<LibrariesView, String> {
    call(command::LIBRARIES, &LibrariesArgs { refresh }).await
}

/// The browser's list of symbols or footprints.
pub async fn library_items(request: LibraryItemsRequest) -> Result<LibraryItems, String> {
    call(command::LIBRARY_ITEMS, &LibraryItemsArgs { request }).await
}

/// One library item's details.
pub async fn library_item(
    request: LibraryItemRequest,
) -> Result<Option<LibraryItemDetail>, String> {
    call(command::LIBRARY_ITEM, &LibraryItemArgs { request }).await
}

#[derive(Serialize)]
struct DrawingArgs<'a> {
    key: &'a str,
}

/// The app's own drawing of a footprint preview, by its drawing key.
pub async fn footprint_drawing(
    key: &str,
) -> Result<Option<desktop_contract::FootprintDrawing>, String> {
    call(command::FOOTPRINT_DRAWING, &DrawingArgs { key }).await
}

#[derive(Serialize)]
struct Footprint3dArgs {
    request: Footprint3dRequest,
}

/// Makes a footprint's 3D view, or reads it from the cache.
pub async fn footprint_3d(request: Footprint3dRequest) -> Result<Footprint3dView, String> {
    call(command::FOOTPRINT_3D, &Footprint3dArgs { request }).await
}

/// A 3D view's GLB, as an `ArrayBuffer`: the command sends raw bytes, not
/// JSON.
pub async fn footprint_glb(key: &str) -> Result<JsValue, String> {
    let serializer = serde_wasm_bindgen::Serializer::json_compatible();
    let args = DrawingArgs { key }
        .serialize(&serializer)
        .map_err(|error| error.to_string())?;
    let bytes = tauri_invoke(command::FOOTPRINT_GLB, args)
        .await
        .map_err(|error| error.as_string().unwrap_or_else(|| format!("{error:?}")))?;
    if bytes.is_instance_of::<js_sys::ArrayBuffer>() {
        Ok(bytes)
    } else {
        Err("The 3D view did not arrive as bytes.".into())
    }
}

/// One library item's drawing.
pub async fn library_preview(request: LibraryItemRequest) -> Result<PreviewOutcome, String> {
    call(command::LIBRARY_PREVIEW, &LibraryItemArgs { request }).await
}

#[derive(Serialize)]
struct PickArgs {
    request: TeamPickRequest,
}

/// What choosing an item in the Libraries browser does for a part.
pub async fn team_pick(request: TeamPickRequest) -> Result<TeamPick, String> {
    call(command::TEAM_PICK, &PickArgs { request }).await
}

/// One of our items drawn as the Catalog has it.
pub async fn team_item_preview(request: LibraryItemRequest) -> Result<PreviewOutcome, String> {
    call(command::TEAM_ITEM_PREVIEW, &LibraryItemArgs { request }).await
}

/// Our libraries' nicknames, by role.
pub async fn our_libraries() -> Result<OurLibraries, String> {
    call(command::OUR_LIBRARIES, &NoArgs {}).await
}

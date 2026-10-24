//! Calls into the Tauri shell. Payload shapes come from `desktop-contract`.

use desktop_contract::{
    AppStatus, FilePreviewRequest, Footprint3dRequest, Footprint3dView, KicadLibrariesStatus,
    LibrariesView, LibraryCheckView, LibraryItemDetail, LibraryItemRequest, LibraryItems,
    LibraryItemsRequest, OurLibraries, PartDetail, PreviewOutcome, PreviewRequest, PublishResult,
    SearchRequest, SearchResponse, SignInRequest, TeamArchiveRequest, TeamChoices,
    TeamCsvReviewRequest, TeamDownload, TeamDownloadRequest, TeamEditRequest, TeamFileReading,
    TeamFileRequest, TeamIpnCheck, TeamIpnRequest, TeamMpnRequest, TeamNewPartRequest, TeamPart,
    TeamPick, TeamPickRequest, TeamSameMpn, TeamsStatus, command,
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

/// A request to one team, named by its server address (blank for none),
/// with the command's own arguments beside it. Every request about a
/// team's parts names its team, so it never depends on which team the
/// view shows.
#[derive(Serialize)]
struct ForTeam<'a, A: Serialize> {
    team: &'a str,
    #[serde(flatten)]
    args: A,
}

/// `args` for the team at `team`.
fn for_team<A: Serialize>(team: &str, args: A) -> ForTeam<'_, A> {
    ForTeam { team, args }
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

/// The part list's search over every signed-in team, or the one team the
/// request names.
pub async fn search(request: SearchRequest) -> Result<SearchResponse, String> {
    call(command::SEARCH_PARTS, &SearchArgs { request }).await
}

/// A part of the team at `team`, or of the CSV catalog alone for a blank
/// `team`.
pub async fn part_detail(team: &str, ipn: &str) -> Result<Option<PartDetail>, String> {
    call(command::PART_DETAIL, &for_team(team, DetailArgs { ipn })).await
}

/// Asks for a part's symbol or footprint preview; see [`PreviewRequest`].
pub async fn preview(team: &str, request: PreviewRequest) -> Result<PreviewOutcome, String> {
    call(
        command::PART_PREVIEW,
        &for_team(team, PreviewArgs { request }),
    )
    .await
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

pub async fn kicad_libraries_status(team: &str) -> Result<KicadLibrariesStatus, String> {
    call(command::KICAD_LIBRARIES_STATUS, &for_team(team, NoArgs {})).await
}

pub async fn kicad_libraries_update(team: &str) -> Result<KicadLibrariesStatus, String> {
    call(command::KICAD_LIBRARIES_UPDATE, &for_team(team, NoArgs {})).await
}

pub async fn kicad_libraries_open_folder(team: &str) -> Result<(), String> {
    call(
        command::KICAD_LIBRARIES_OPEN_FOLDER,
        &for_team(team, NoArgs {}),
    )
    .await
}

#[derive(Serialize)]
struct KindArgs<'a> {
    kind: &'a str,
}

pub async fn kicad_libraries_copied(
    team: &str,
    kind: &str,
) -> Result<KicadLibrariesStatus, String> {
    call(
        command::KICAD_LIBRARIES_COPIED,
        &for_team(team, KindArgs { kind }),
    )
    .await
}

#[derive(Serialize)]
struct CodeArgs<'a> {
    code: &'a str,
}

pub async fn team_code_check(
    team: &str,
    code: &str,
) -> Result<desktop_contract::TeamCodeCheck, String> {
    call(command::TEAM_CODE_CHECK, &for_team(team, CodeArgs { code })).await
}

pub async fn team_code_set(team: &str, code: &str) -> Result<KicadLibrariesStatus, String> {
    call(command::TEAM_CODE_SET, &for_team(team, CodeArgs { code })).await
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

#[derive(Serialize)]
struct ChoicesArgs {
    choices: TeamChoices,
}

/// Remembers what the person chose in the part list, and the team each
/// feature last used, for the next start.
pub async fn team_choose(choices: TeamChoices) -> Result<TeamsStatus, String> {
    call(command::TEAM_CHOOSE, &ChoicesArgs { choices }).await
}

#[derive(Serialize)]
struct UseNewServerArgs {
    request: desktop_contract::UseNewServerRequest,
}

/// The person said a team moved to its server, so its KiCad libraries
/// take updates from it.
pub async fn kicad_libraries_use_new_server(
    request: desktop_contract::UseNewServerRequest,
) -> Result<TeamsStatus, String> {
    call(
        command::KICAD_LIBRARIES_USE_NEW_SERVER,
        &UseNewServerArgs { request },
    )
    .await
}

#[derive(Serialize)]
struct LibraryRepoArgs {
    request: desktop_contract::LibraryRepoRequest,
}

/// Links a team to its library repo, from a folder or an address.
pub async fn library_repo_link(
    request: desktop_contract::LibraryRepoRequest,
) -> Result<desktop_contract::LibraryRepoLinking, String> {
    call(command::LIBRARY_REPO_LINK, &LibraryRepoArgs { request }).await
}

/// Checks a team's library repo again, in the background.
pub async fn library_repo_check(server_url: &str) -> Result<TeamsStatus, String> {
    call(command::LIBRARY_REPO_CHECK, &ServerArgs { server_url }).await
}

/// Unlinks a team from its library repo.
pub async fn library_repo_unlink(server_url: &str) -> Result<TeamsStatus, String> {
    call(command::LIBRARY_REPO_UNLINK, &ServerArgs { server_url }).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LayoutReviewArgs<'a> {
    server_url: &'a str,
    fetch: bool,
}

/// Scans a team's library repo for its library layout, to review; with
/// `fetch`, the app's copy is brought up to date first.
pub async fn library_layout_review(
    server_url: &str,
    fetch: bool,
) -> Result<desktop_contract::LibraryLayoutReview, String> {
    call(
        command::LIBRARY_LAYOUT_REVIEW,
        &LayoutReviewArgs { server_url, fetch },
    )
    .await
}

#[derive(Serialize)]
struct LayoutUseArgs {
    request: desktop_contract::LayoutUseRequest,
}

/// Keeps the library layout the person reviewed.
pub async fn library_layout_use(
    request: desktop_contract::LayoutUseRequest,
) -> Result<TeamsStatus, String> {
    call(command::LIBRARY_LAYOUT_USE, &LayoutUseArgs { request }).await
}

pub async fn team_part(team: &str, ipn: &str) -> Result<Option<TeamPart>, String> {
    call(command::TEAM_PART, &for_team(team, DetailArgs { ipn })).await
}

pub async fn team_edit(team: &str, request: TeamEditRequest) -> Result<PublishResult, String> {
    call(command::TEAM_EDIT, &for_team(team, EditArgs { request })).await
}

pub async fn team_archive(
    team: &str,
    request: TeamArchiveRequest,
) -> Result<PublishResult, String> {
    call(
        command::TEAM_ARCHIVE,
        &for_team(team, ArchiveArgs { request }),
    )
    .await
}

pub async fn team_csv_review(
    team: &str,
    request: TeamCsvReviewRequest,
) -> Result<PublishResult, String> {
    call(
        command::TEAM_CSV_REVIEW,
        &for_team(team, CsvReviewArgs { request }),
    )
    .await
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
pub async fn team_read_file(
    team: &str,
    request: TeamFileRequest,
) -> Result<TeamFileReading, String> {
    call(
        command::TEAM_READ_FILE,
        &for_team(team, ReadFileArgs { request }),
    )
    .await
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
pub async fn team_read_download(
    team: &str,
    request: TeamDownloadRequest,
) -> Result<TeamDownload, String> {
    call(
        command::TEAM_READ_DOWNLOAD,
        &for_team(team, DownloadArgs { request }),
    )
    .await
}

/// Whether an IPN can be a new part's in the team at `team`.
pub async fn team_check_ipn(
    team: &str,
    ipn: String,
    prefix: String,
) -> Result<TeamIpnCheck, String> {
    call(
        command::TEAM_CHECK_IPN,
        &for_team(
            team,
            IpnArgs {
                request: TeamIpnRequest { ipn, prefix },
            },
        ),
    )
    .await
}

/// Saves a new part in the team at `team`.
pub async fn team_add_part(
    team: &str,
    request: TeamNewPartRequest,
) -> Result<PublishResult, String> {
    call(
        command::TEAM_ADD_PART,
        &for_team(team, NewPartArgs { request }),
    )
    .await
}

#[derive(Serialize)]
struct MpnArgs {
    request: TeamMpnRequest,
}

/// The parts of the team at `team` that already have an MPN.
pub async fn team_check_mpn(
    team: &str,
    mpn: String,
    manufacturer: String,
) -> Result<Vec<TeamSameMpn>, String> {
    call(
        command::TEAM_CHECK_MPN,
        &for_team(
            team,
            MpnArgs {
                request: TeamMpnRequest { mpn, manufacturer },
            },
        ),
    )
    .await
}

/// What the library check against the team at `team` found, for someone
/// who can edit there; `None` for anyone else. The app works it out again
/// only when that team's catalog changed.
pub async fn team_library_check(team: &str) -> Result<Option<LibraryCheckView>, String> {
    call(command::TEAM_LIBRARY_CHECK, &for_team(team, NoArgs {})).await
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

/// The Libraries browser's items. One team's parts mark the items they use
/// when `team` names it, else every signed-in team's parts.
#[derive(Serialize)]
struct LibraryArgs<'a, A: Serialize> {
    team: Option<&'a str>,
    #[serde(flatten)]
    args: A,
}

/// The browser's list of symbols or footprints, marking those the parts of
/// `team` use (every team's when `None`).
pub async fn library_items(
    team: Option<&str>,
    request: LibraryItemsRequest,
) -> Result<LibraryItems, String> {
    call(
        command::LIBRARY_ITEMS,
        &LibraryArgs {
            team,
            args: LibraryItemsArgs { request },
        },
    )
    .await
}

/// One library item's details, with the parts of `team` that use it
/// (every team's when `None`).
pub async fn library_item(
    team: Option<&str>,
    request: LibraryItemRequest,
) -> Result<Option<LibraryItemDetail>, String> {
    call(
        command::LIBRARY_ITEM,
        &LibraryArgs {
            team,
            args: LibraryItemArgs { request },
        },
    )
    .await
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

/// What choosing an item in the Libraries browser does for a part of the
/// team at `team`.
pub async fn team_pick(team: &str, request: TeamPickRequest) -> Result<TeamPick, String> {
    call(command::TEAM_PICK, &for_team(team, PickArgs { request })).await
}

/// One of our items drawn as the Catalog of the team at `team` has it.
pub async fn team_item_preview(
    team: &str,
    request: LibraryItemRequest,
) -> Result<PreviewOutcome, String> {
    call(
        command::TEAM_ITEM_PREVIEW,
        &for_team(team, LibraryItemArgs { request }),
    )
    .await
}

/// Our libraries' nicknames, by role.
pub async fn our_libraries() -> Result<OurLibraries, String> {
    call(command::OUR_LIBRARIES, &NoArgs {}).await
}

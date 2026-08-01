use semantic_engine_source_runtime::{
    BrowserAuthorizationPrompt, DeviceAuthorizationPrompt, SourceRuntime, SourceView,
    TwitchAuthorizationStatus, TwitchSourceTest, YouTubeAuthorizationStatus, YouTubeSourceTest,
};
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub async fn list_sources_ipc(
    state: State<'_, Arc<SourceRuntime>>,
) -> Result<Vec<SourceView>, String> {
    semantic_engine_source_runtime::list_sources(state.inner().as_ref()).await
}

#[tauri::command]
pub async fn create_youtube_source_ipc(
    display_name: String,
    client_id: String,
    video_id: String,
    policy_acknowledged: bool,
    state: State<'_, Arc<SourceRuntime>>,
) -> Result<SourceView, String> {
    semantic_engine_source_runtime::create_youtube_source(
        display_name,
        client_id,
        video_id,
        policy_acknowledged,
        state.inner().as_ref(),
    )
    .await
}

#[tauri::command]
pub async fn begin_youtube_authorization_ipc(
    source_id: String,
    state: State<'_, Arc<SourceRuntime>>,
) -> Result<BrowserAuthorizationPrompt, String> {
    semantic_engine_source_runtime::begin_youtube_authorization(source_id, state.inner().as_ref())
        .await
}

#[tauri::command]
pub async fn poll_youtube_authorization_ipc(
    source_id: String,
    state: State<'_, Arc<SourceRuntime>>,
) -> Result<YouTubeAuthorizationStatus, String> {
    semantic_engine_source_runtime::poll_youtube_authorization(source_id, state.inner().as_ref())
        .await
}

#[tauri::command]
pub async fn test_youtube_source_ipc(
    source_id: String,
    state: State<'_, Arc<SourceRuntime>>,
) -> Result<YouTubeSourceTest, String> {
    semantic_engine_source_runtime::test_youtube_source(source_id, state.inner().as_ref()).await
}

#[tauri::command]
pub async fn start_youtube_source_ipc(
    source_id: String,
    expected_revision: u64,
    session_id: String,
    state: State<'_, Arc<SourceRuntime>>,
) -> Result<SourceView, String> {
    semantic_engine_source_runtime::start_youtube_source(
        source_id,
        expected_revision,
        session_id,
        state.inner().as_ref(),
    )
    .await
}

#[tauri::command]
pub fn open_youtube_authorization_ipc(authorization_uri: String) -> Result<(), String> {
    let url = tauri::Url::parse(&authorization_uri)
        .map_err(|_| "authorization URL is invalid".to_owned())?;
    if url.scheme() != "https"
        || url.host_str() != Some("accounts.google.com")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("authorization URL is not a trusted Google endpoint".to_owned());
    }
    open_system_browser(url.as_str())
}

#[cfg(target_os = "windows")]
fn open_system_browser(url: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("system browser could not be opened: {error}"))
}

#[cfg(target_os = "macos")]
fn open_system_browser(url: &str) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("system browser could not be opened: {error}"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_system_browser(url: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("system browser could not be opened: {error}"))
}

#[tauri::command]
pub async fn create_twitch_source_ipc(
    display_name: String,
    client_id: String,
    state: State<'_, Arc<SourceRuntime>>,
) -> Result<SourceView, String> {
    semantic_engine_source_runtime::create_twitch_source(
        display_name,
        client_id,
        state.inner().as_ref(),
    )
    .await
}

#[tauri::command]
pub async fn begin_twitch_authorization_ipc(
    source_id: String,
    state: State<'_, Arc<SourceRuntime>>,
) -> Result<DeviceAuthorizationPrompt, String> {
    semantic_engine_source_runtime::begin_twitch_authorization(source_id, state.inner().as_ref())
        .await
}

#[tauri::command]
pub async fn poll_twitch_authorization_ipc(
    source_id: String,
    state: State<'_, Arc<SourceRuntime>>,
) -> Result<TwitchAuthorizationStatus, String> {
    semantic_engine_source_runtime::poll_twitch_authorization(source_id, state.inner().as_ref())
        .await
}

#[tauri::command]
pub async fn test_twitch_source_ipc(
    source_id: String,
    state: State<'_, Arc<SourceRuntime>>,
) -> Result<TwitchSourceTest, String> {
    semantic_engine_source_runtime::test_twitch_source(source_id, state.inner().as_ref()).await
}

#[tauri::command]
pub async fn start_twitch_source_ipc(
    source_id: String,
    expected_revision: u64,
    session_id: String,
    state: State<'_, Arc<SourceRuntime>>,
) -> Result<SourceView, String> {
    semantic_engine_source_runtime::start_twitch_source(
        source_id,
        expected_revision,
        session_id,
        state.inner().as_ref(),
    )
    .await
}

#[tauri::command]
pub async fn stop_source_ipc(
    source_id: String,
    state: State<'_, Arc<SourceRuntime>>,
) -> Result<SourceView, String> {
    semantic_engine_source_runtime::stop_source(source_id, state.inner().as_ref()).await
}

#[tauri::command]
pub async fn delete_source_ipc(
    source_id: String,
    expected_revision: u64,
    state: State<'_, Arc<SourceRuntime>>,
) -> Result<(), String> {
    semantic_engine_source_runtime::delete_source(
        source_id,
        expected_revision,
        state.inner().as_ref(),
    )
    .await
}

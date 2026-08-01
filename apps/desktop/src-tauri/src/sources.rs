use semantic_engine_source_runtime::{
    DeviceAuthorizationPrompt, SourceRuntime, SourceView, TwitchAuthorizationStatus,
    TwitchSourceTest,
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

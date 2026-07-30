use semantic_engine_core::{Round, Submission, Validation, Validator};

#[tauri::command]
fn validate(round: Round, submission: Submission) -> Validation {
    Validator::default().validate(&round, &submission)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![validate])
        .run(tauri::generate_context!())
        .expect("error while running Semantic Engine");
}

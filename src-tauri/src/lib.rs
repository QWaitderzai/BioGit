//! Точка входа приложения BioGit (Tauri v2).
//!
//! Здесь регистрируются команды, которые фронтенд вызывает через `invoke()`.

mod commands;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::open_repo,
            commands::init_repo,
            commands::scan_files,
            commands::save_snapshot,
            commands::list_history,
            commands::restore_snapshot,
            commands::list_branches,
            commands::create_branch,
            commands::switch_branch,
            commands::merge_branch,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("Не удалось запустить BioGit: {e}");
            std::process::exit(1);
        });
}

//! Точка входа приложения BioGit (Tauri v2).
//!
//! Здесь регистрируются команды, которые фронтенд вызывает через `invoke()`.

mod commands;

use std::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::SheetState(Mutex::new(None)))
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
            commands::pick_folder,
            commands::read_file,
            commands::write_file,
            // статус и diff
            commands::file_status,
            commands::diff_working,
            commands::diff_between,
            commands::history_graph_cmd,
            // двухфазное слияние
            commands::merge_propose,
            commands::merge_apply,
            // Excel (уровень A) + встроенный fallback
            commands::excel_status,
            commands::sheet_open,
            commands::sheet_set_active,
            commands::sheet_edit,
            commands::sheet_save,
            commands::sheet_close_cmd,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("Не удалось запустить BioGit: {e}");
            std::process::exit(1);
        });
}

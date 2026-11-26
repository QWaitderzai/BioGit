//! Команды Tauri: тонкие обёртки над ядром `biogit-core`.
//!
//! Каждая команда возвращает `Result<T, String>`: ошибки ядра сериализуются в строку.

use std::path::Path;
use std::sync::Mutex;

use biogit_core::{
    apply_merge, diff_snapshots, diff_working_vs_snapshot, history_graph, propose_merge,
    working_status, EditOutcome, ExcelAvailability, FileDecision, FileDiff, FileStatus, GraphData,
    MergeOptions, MergeProposal, MergeReport, Repo, SheetDoc, SheetSession,
};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

/// Полное состояние репозитория для интерфейса.
#[derive(serde::Serialize)]
pub struct RepoState {
    pub path: String,
    pub current_branch: String,
    pub branches: Vec<biogit_core::BranchInfo>,
    pub files: Vec<biogit_core::FileEntry>,
    pub latest: Option<biogit_core::Snapshot>,
}

/// Состояние открытой книги (Excel-сессия) — одна на приложение.
pub struct SheetState(pub Mutex<Option<SheetSession>>);

fn load_state(path: &str) -> Result<RepoState, String> {
    let repo = Repo::open(Path::new(path)).map_err(|e| e.to_string())?;
    let current_branch = repo.head().map_err(|e| e.to_string())?;
    let branches = repo.list_branches().map_err(|e| e.to_string())?;
    let files = repo.list_files().map_err(|e| e.to_string())?;
    let latest = repo.current_commit().map_err(|e| e.to_string())?;
    Ok(RepoState {
        path: path.to_string(),
        current_branch,
        branches,
        files,
        latest,
    })
}

fn open(path: &str) -> Result<Repo, String> {
    Repo::open(Path::new(path)).map_err(|e| e.to_string())
}

/// Открывает существующий репозиторий.
#[tauri::command]
pub async fn open_repo(path: String) -> Result<RepoState, String> {
    load_state(&path)
}

/// Создаёт новый репозиторий BioGit в указанной папке.
#[tauri::command]
pub async fn init_repo(path: String) -> Result<RepoState, String> {
    Repo::init(Path::new(&path)).map_err(|e| e.to_string())?;
    load_state(&path)
}

/// Список файлов рабочей папки.
#[tauri::command]
pub async fn scan_files(path: String) -> Result<Vec<biogit_core::FileEntry>, String> {
    open(&path)?.list_files().map_err(|e| e.to_string())
}

/// Сохраняет снимок: `parameters_json` — текст JSON с параметрами.
#[tauri::command]
pub async fn save_snapshot(
    path: String,
    message: String,
    author: String,
    parameters_json: Option<String>,
) -> Result<biogit_core::Snapshot, String> {
    let repo = open(&path)?;
    let parameters: serde_json::Value = match parameters_json {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(&text)
            .map_err(|e| format!("Некорректный JSON параметров: {e}"))?,
        _ => serde_json::Value::Null,
    };
    repo.save_snapshot(&message, &author, &parameters)
        .map_err(|e| e.to_string())
}

/// История снимков ветки (пустое имя — текущая ветка).
#[tauri::command]
pub async fn list_history(
    path: String,
    branch: Option<String>,
) -> Result<Vec<biogit_core::Snapshot>, String> {
    let repo = open(&path)?;
    repo.history(branch.as_deref().unwrap_or(""))
        .map_err(|e| e.to_string())
}

/// восстанавливает файлы из снимка. Возвращает число файлов.
#[tauri::command]
pub async fn restore_snapshot(path: String, hash: String) -> Result<usize, String> {
    open(&path)?.restore(&hash).map_err(|e| e.to_string())
}

/// Список ветвей гипотез.
#[tauri::command]
pub async fn list_branches(path: String) -> Result<Vec<biogit_core::BranchInfo>, String> {
    open(&path)?.list_branches().map_err(|e| e.to_string())
}

/// Создаёт ветвь гипотезы от текущего состояния.
#[tauri::command]
pub async fn create_branch(path: String, name: String) -> Result<(), String> {
    open(&path)?.create_branch(&name).map_err(|e| e.to_string())
}

/// Переключается на ветку и восстанавливает её файлы.
#[tauri::command]
pub async fn switch_branch(path: String, name: String) -> Result<(), String> {
    open(&path)?.switch_branch(&name).map_err(|e| e.to_string())
}

/// Сливает ветку-гипотезу `source` в текущую ветку (старое поведение:
/// конфликт отменяет слияние целиком).
#[tauri::command]
pub async fn merge_branch(path: String, source: String) -> Result<MergeReport, String> {
    let repo = open(&path)?;
    biogit_core::merge_branch(&repo, &source).map_err(|e| e.to_string())
}

/// Диалог выбора папки; `None` — пользователь передумал.
#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    let (sender, mut receiver) = tauri::async_runtime::channel::<Option<String>>(1);
    app.dialog().file().pick_folder(move |file_path| {
        let msg = file_path.map(|fp| fp.to_string());
        tauri::async_runtime::spawn(async move {
            let _ = sender.send(msg).await;
        });
    });
    match receiver.recv().await {
        Some(chosen) => Ok(chosen),
        None => Err("канал диалога выбора папки закрыт".to_string()),
    }
}

/// Читает файл рабочей папки для редактора (текст, двоичность, размер).
#[tauri::command]
pub async fn read_file(path: String, name: String) -> Result<biogit_core::FileContent, String> {
    open(&path)?
        .read_work_file(&name)
        .map_err(|e| e.to_string())
}

/// Пишет текст в файл рабочей папки, возвращает новый размер.
#[tauri::command]
pub async fn write_file(path: String, name: String, content: String) -> Result<u64, String> {
    open(&path)?
        .write_work_file(&name, &content)
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Статус и diff
// ---------------------------------------------------------------------------

/// Статус рабочих файлов относительно последнего снимка (изменён/новый/удалён).
#[tauri::command]
pub async fn file_status(path: String) -> Result<Vec<FileStatus>, String> {
    let repo = open(&path)?;
    working_status(&repo).map_err(|e| e.to_string())
}

/// Diff рабочей папки против снимка (`hash` — пустая строка для последнего снимка).
#[tauri::command]
pub async fn diff_working(path: String, hash: Option<String>) -> Result<Vec<FileDiff>, String> {
    let repo = open(&path)?;
    let hash_ref = hash.as_deref().filter(|h| !h.trim().is_empty());
    diff_working_vs_snapshot(&repo, hash_ref).map_err(|e| e.to_string())
}

/// Diff между двумя снимками.
#[tauri::command]
pub async fn diff_between(path: String, a: String, b: String) -> Result<Vec<FileDiff>, String> {
    let repo = open(&path)?;
    diff_snapshots(&repo, &a, &b).map_err(|e| e.to_string())
}

/// Граф истории: все снимки всех ветвей с раскладкой линий.
#[tauri::command]
pub async fn history_graph_cmd(path: String) -> Result<GraphData, String> {
    let repo = open(&path)?;
    history_graph(&repo).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Двухфазное слияние с разрешением конфликтов
// ---------------------------------------------------------------------------

/// Фаза 1: предложение слияния (без записи; содержит структурированные конфликты).
#[tauri::command]
pub async fn merge_propose(
    path: String,
    source: String,
    csv_key: Option<String>,
) -> Result<MergeProposal, String> {
    let repo = open(&path)?;
    let options = MergeOptions { csv_key };
    propose_merge(&repo, &source, &options).map_err(|e| e.to_string())
}

/// Фаза 2: применяет слияние с решениями пользователя по конфликтам.
#[tauri::command]
pub async fn merge_apply(
    path: String,
    source: String,
    csv_key: Option<String>,
    decisions: Vec<FileDecision>,
) -> Result<MergeReport, String> {
    let repo = open(&path)?;
    let options = MergeOptions { csv_key };
    apply_merge(&repo, &source, &options, &decisions).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Excel (уровень A) с fallback во встроенный режим
// ---------------------------------------------------------------------------

/// Проверяет доступность Excel на машине (запускает мост, пингует).
#[tauri::command]
pub async fn excel_status() -> Result<ExcelAvailability, String> {
    Ok(biogit_core::excel_probe())
}

/// Открывает книгу: настоящий Excel через COM либо встроенный режим.
#[tauri::command]
pub async fn sheet_open(
    path: String,
    name: String,
    state: State<'_, SheetState>,
) -> Result<SheetDoc, String> {
    let repo = open(&path)?;
    let session = biogit_core::sheet_open(&repo, &name).map_err(|e| e.to_string())?;
    let doc = session.doc.clone();
    let mut guard = state.0.lock().map_err(|_| "состояние книги заблокировано".to_string())?;
    // закрываем предыдущую сессию, если была
    if let Some(old) = guard.as_mut() {
        biogit_core::sheet_close(old);
    }
    *guard = Some(session);
    Ok(doc)
}

/// Переключает активный лист открытой книги.
#[tauri::command]
pub async fn sheet_set_active(
    sheet: usize,
    state: State<'_, SheetState>,
) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|_| "состояние книги заблокировано".to_string())?;
    let session = guard
        .as_mut()
        .ok_or_else(|| "книга не открыта".to_string())?;
    biogit_core::sheet_set_active(session, sheet).map_err(|e| e.to_string())
}

/// Редактирует ячейку открытой книги.
#[tauri::command]
pub async fn sheet_edit(
    sheet: usize,
    row: usize,
    col: usize,
    input: String,
    state: State<'_, SheetState>,
) -> Result<EditOutcome, String> {
    let mut guard = state.0.lock().map_err(|_| "состояние книги заблокировано".to_string())?;
    let session = guard
        .as_mut()
        .ok_or_else(|| "книга не открыта".to_string())?;
    biogit_core::sheet_edit(session, sheet, row, col, &input).map_err(|e| e.to_string())
}

/// Сохраняет книгу; возвращает актуальное имя файла (builtin может сменить .xls → .xlsx).
#[tauri::command]
pub async fn sheet_save(
    path: String,
    state: State<'_, SheetState>,
) -> Result<String, String> {
    let repo = open(&path)?;
    let mut guard = state.0.lock().map_err(|_| "состояние книги заблокировано".to_string())?;
    let session = guard
        .as_mut()
        .ok_or_else(|| "книга не открыта".to_string())?;
    biogit_core::sheet_save(session, &repo).map_err(|e| e.to_string())
}

/// Закрывает книгу (Excel: закрывает книгу и завершает процесс Excel).
#[tauri::command]
pub async fn sheet_close_cmd(state: State<'_, SheetState>) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|_| "состояние книги заблокировано".to_string())?;
    if let Some(session) = guard.as_mut() {
        biogit_core::sheet_close(session);
    }
    *guard = None;
    Ok(())
}

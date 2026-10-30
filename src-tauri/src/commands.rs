//! Команды Tauri: тонкие обёртки над ядром `biogit-core`.
//!
//! Каждая команда возвращает `Result<T, String>`: ошибки ядра сериализуются

use std::path::Path;

use biogit_core::{BranchInfo, FileEntry, MergeReport, Repo, Snapshot};

/// Полное состояние репозитория для интерфейса.
#[derive(serde::Serialize)]
pub struct RepoState {
    pub path: String,
    pub current_branch: String,
    pub branches: Vec<BranchInfo>,
    pub files: Vec<FileEntry>,
    pub latest: Option<Snapshot>,
}

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
pub async fn scan_files(path: String) -> Result<Vec<FileEntry>, String> {
    open(&path)?.list_files().map_err(|e| e.to_string())
}

/// Сохраняет снимок: `parameters_json` — текст JSON с параметрами.
#[tauri::command]
pub async fn save_snapshot(
    path: String,
    message: String,
    author: String,
    parameters_json: Option<String>,
) -> Result<Snapshot, String> {
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
pub async fn list_history(path: String, branch: Option<String>) -> Result<Vec<Snapshot>, String> {
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
pub async fn list_branches(path: String) -> Result<Vec<BranchInfo>, String> {
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

/// Сливает ветку-гипотезу `source` в текущую ветку.
#[tauri::command]
pub async fn merge_branch(path: String, source: String) -> Result<MergeReport, String> {
    let repo = open(&path)?;
    biogit_core::merge_branch(&repo, &source).map_err(|e| e.to_string())
}

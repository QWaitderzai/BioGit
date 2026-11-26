//! Сравнение состояний: статус рабочих файлов и diff между снимками.
//!
//! Два сценария:
//! 1. [`working_status`] — какие файлы рабочей папки изменены/добавлены/удалены
//!    относительно последнего снимка (индикаторы в проводнике);
//! 2. [`diff_snapshots`] / [`diff_working_vs_snapshot`] — содержательные отличия
//!    (построчные для текста, поячеечные для CSV) для просмотра изменений.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{BioGitError, Result};
use crate::object::{Snapshot, Tree};
use crate::snapshot::Repo;

/// Максимальное произведение размеров сравниваемых файлов для построчного diff
/// (защита от гигантских файлов, как в `merge.rs`).
const MAX_DIFF_MATRIX: usize = 1_000_000;

/// Лимит клеток CSV в diff — больше считаем «слишком большой таблицей».
const MAX_CSV_CELLS: usize = 200_000;

// ---------------------------------------------------------------------------
// Статус рабочей папки
// ---------------------------------------------------------------------------

/// Состояние одного файла относительно последнего снимка ветки.
#[derive(Debug, Clone, Serialize)]
pub struct FileStatus {
    /// Относительный путь, разделитель «/».
    pub name: String,
    /// "unmodified" | "modified" | "added" | "deleted"
    pub state: String,
    /// Текущий размер файла (0 у удалённых).
    pub size: u64,
    /// Размер файла в последнем снимке (у добавленных — `None`).
    pub previous_size: Option<u64>,
}

/// Считает SHA-256 содержимого файла, не записывая объект в хранилище.
fn hash_file_content(path: &Path) -> Result<String> {
    let file = fs::File::open(path)?;
    let mut reader = std::io::BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Статус всех файлов рабочей папки относительно последнего снимка текущей ветки.
///
/// Список включает и удалённые файлы (они есть в снимке, но не на диске) —
/// интерфейс показывает их отдельной группой с пометкой «удалён».
pub fn working_status(repo: &Repo) -> Result<Vec<FileStatus>> {
    let head: Option<Snapshot> = repo.current_commit()?;
    let tree: Tree = match &head {
        Some(snap) => repo.tree_of(snap)?,
        None => Tree::default(),
    };
    let committed: BTreeMap<&str, &crate::object::TreeEntry> = tree
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e))
        .collect();

    let mut out: Vec<FileStatus> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for file in repo.list_files()? {
        seen.insert(file.name.clone());
        let entry = committed.get(file.name.as_str());
        let state = match entry {
            None => "added",
            Some(old) => {
                let current_hash = hash_file_content(&repo.root().join(&file.name))?;
                if current_hash == old.hash {
                    "unmodified"
                } else {
                    "modified"
                }
            }
        };
        out.push(FileStatus {
            name: file.name,
            state: state.to_string(),
            size: file.size,
            previous_size: entry.map(|e| e.size),
        });
    }

    // удалённые: есть в снимке, но нет в рабочей папке
    for (name, entry) in &committed {
        if !seen.contains(*name) {
            out.push(FileStatus {
                name: (*name).to_string(),
                state: "deleted".to_string(),
                size: 0,
                previous_size: Some(entry.size),
            });
        }
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

// ---------------------------------------------------------------------------
// Diff содержимого
// ---------------------------------------------------------------------------

/// Одна строка построчного diff.
#[derive(Debug, Clone, Serialize)]
pub struct DiffLine {
    /// "equal" | "plus" | "minus"
    pub kind: String,
    pub text: String,
}

/// Изменение одной ячейки CSV.
#[derive(Debug, Clone, Serialize)]
pub struct CsvCellChange {
    pub key: String,
    pub column: String,
    pub old: String,
    pub new: String,
}

/// Diff CSV-файла по ключевой колонке (первая).
#[derive(Debug, Clone, Serialize)]
pub struct CsvDiff {
    pub header: Vec<String>,
    /// Полные добавленные строки.
    pub added: Vec<Vec<String>>,
    /// Полные удалённые строки.
    pub removed: Vec<Vec<String>>,
    /// Изменённые ячейки строк, присутствующих с обеих сторон.
    pub changed: Vec<CsvCellChange>,
}

/// Diff одного файла между двумя деревьями.
#[derive(Debug, Clone, Serialize)]
pub struct FileDiff {
    pub path: String,
    /// "added" | "deleted" | "modified"
    pub state: String,
    /// "text" | "csv" | "binary"
    pub kind: String,
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
    /// Построчный diff (только для текста).
    pub lines: Option<Vec<DiffLine>>,
    /// Табличный diff (только для CSV/TSV).
    pub csv: Option<CsvDiff>,
}

/// Классификация файла по расширению для выбора способа diff.
fn file_kind(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "csv" | "tsv" => "csv",
        _ => "text",
    }
}

/// Построчный diff двух текстов через LCS (переиспользует `lcs_matches` из merge).
pub(crate) fn line_diff(base: &str, other: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = base.split_inclusive('\n').collect();
    let b: Vec<&str> = other.split_inclusive('\n').collect();
    let mut out = Vec::new();
    if a.len().saturating_mul(b.len()) > MAX_DIFF_MATRIX {
        // слишком большой файл — грубый diff: «все строки изменены»
        for line in a {
            out.push(DiffLine { kind: "minus".into(), text: line.to_string() });
        }
        for line in b {
            out.push(DiffLine { kind: "plus".into(), text: line.to_string() });
        }
        return out;
    }
    let owned_a: Vec<String> = a.iter().map(|s| (*s).to_string()).collect();
    let owned_b: Vec<String> = b.iter().map(|s| (*s).to_string()).collect();
    let matches = crate::merge::lcs_matches(&owned_a, &owned_b);
    let mut ai = 0usize;
    let mut bi = 0usize;
    for &(i, j) in &matches {
        while ai < i {
            out.push(DiffLine { kind: "minus".into(), text: owned_a[ai].clone() });
            ai += 1;
        }
        while bi < j {
            out.push(DiffLine { kind: "plus".into(), text: owned_b[bi].clone() });
            bi += 1;
        }
        out.push(DiffLine { kind: "equal".into(), text: owned_a[i].clone() });
        ai += 1;
        bi += 1;
    }
    while ai < owned_a.len() {
        out.push(DiffLine { kind: "minus".into(), text: owned_a[ai].clone() });
        ai += 1;
    }
    while bi < owned_b.len() {
        out.push(DiffLine { kind: "plus".into(), text: owned_b[bi].clone() });
        bi += 1;
    }
    out
}

/// Табличный diff CSV по ключевой (первой) колонке.
pub(crate) fn csv_diff(old_bytes: &[u8], new_bytes: &[u8]) -> Result<CsvDiff> {
    let old_table = crate::merge::read_csv_table(old_bytes, "старая версия")?;
    let new_table = crate::merge::read_csv_table(new_bytes, "новая версия")?;
    let (header, old_rows) = old_table;
    let (_, new_rows) = new_table;

    if header.is_empty() {
        return Err(BioGitError::Csv("пустой CSV".to_string()));
    }
    if header.len().saturating_mul(old_rows.len().max(new_rows.len())) > MAX_CSV_CELLS {
        return Err(BioGitError::Csv(
            "таблица слишком велика для табличного diff".to_string(),
        ));
    }

    let to_map = |rows: &[Vec<String>]| -> BTreeMap<String, Vec<String>> {
        rows.iter()
            .map(|row| (row.first().cloned().unwrap_or_default(), row.clone()))
            .collect()
    };
    let old_map = to_map(&old_rows);
    let new_map = to_map(&new_rows);

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    for (key, row) in &new_map {
        match old_map.get(key) {
            None => added.push(row.clone()),
            Some(old_row) => {
                for (c, column) in header.iter().enumerate() {
                    let old_val = old_row.get(c).map(String::as_str).unwrap_or("");
                    let new_val = row.get(c).map(String::as_str).unwrap_or("");
                    if old_val != new_val {
                        changed.push(CsvCellChange {
                            key: key.clone(),
                            column: column.clone(),
                            old: old_val.to_string(),
                            new: new_val.to_string(),
                        });
                    }
                }
            }
        }
    }
    for (key, row) in &old_map {
        if !new_map.contains_key(key) {
            removed.push(row.clone());
        }
    }
    Ok(CsvDiff { header, added, removed, changed })
}

/// Diff содержимого одного файла по двум блобам (`None` — файла нет с этой стороны).
fn content_diff(
    repo: &Repo,
    path: &str,
    old: Option<(&str, u64)>,
    new: Option<(&str, u64)>,
) -> Result<FileDiff> {
    let old_bytes = match old {
        Some((hash, _)) => Some(repo.read_blob(hash)?),
        None => None,
    };
    let new_bytes = match new {
        Some((hash, _)) => Some(repo.read_blob(hash)?),
        None => None,
    };
    diff_bytes(path, old, new, old_bytes, new_bytes)
}

/// Diff по готовым байтам: `old`/`new` — кортежи (хеш, размер) для метаданных.
fn diff_bytes(
    path: &str,
    old_meta: Option<(&str, u64)>,
    new_meta: Option<(&str, u64)>,
    old_bytes: Option<Vec<u8>>,
    new_bytes: Option<Vec<u8>>,
) -> Result<FileDiff> {
    let (state, kind) = match (old_meta, new_meta) {
        (None, Some(_)) => ("added", file_kind(path)),
        (Some(_), None) => ("deleted", file_kind(path)),
        (Some(_), Some(_)) => ("modified", file_kind(path)),
        (None, None) => unreachable!("файл не может отсутствовать с обеих сторон"),
    };
    let old_size = old_meta.map(|(_, s)| s);
    let new_size = new_meta.map(|(_, s)| s);

    let mut diff = FileDiff {
        path: path.to_string(),
        state: state.to_string(),
        kind: kind.to_string(),
        old_size,
        new_size,
        lines: None,
        csv: None,
    };

    match state {
        "modified" => {
            let old_bytes = old_bytes.unwrap_or_default();
            let new_bytes = new_bytes.unwrap_or_default();
            match kind {
                "csv" => {
                    diff.csv = Some(csv_diff(&old_bytes, &new_bytes)?);
                }
                _ => {
                    let old_text = String::from_utf8_lossy(&old_bytes);
                    let new_text = String::from_utf8_lossy(&new_bytes);
                    diff.lines = Some(line_diff(&old_text, &new_text));
                }
            }
        }
        // добавленные/удалённые файлы целиком показываются как +/− строки
        "added" => {
            let bytes = new_bytes.unwrap_or_default();
            if file_kind(path) == "csv" {
                let table = crate::merge::read_csv_table(&bytes, "новая версия")?;
                diff.csv = Some(CsvDiff {
                    header: table.0,
                    added: table.1,
                    removed: Vec::new(),
                    changed: Vec::new(),
                });
            } else if let Ok(text) = String::from_utf8(bytes) {
                diff.lines = Some(
                    text.split_inclusive('\n')
                        .map(|line| DiffLine { kind: "plus".into(), text: line.to_string() })
                        .collect(),
                );
            } else {
                diff.kind = "binary".to_string();
            }
        }
        "deleted" => {
            let bytes = old_bytes.unwrap_or_default();
            if file_kind(path) == "csv" {
                let table = crate::merge::read_csv_table(&bytes, "старая версия")?;
                diff.csv = Some(CsvDiff {
                    header: table.0,
                    added: Vec::new(),
                    removed: table.1,
                    changed: Vec::new(),
                });
            } else if let Ok(text) = String::from_utf8(bytes) {
                diff.lines = Some(
                    text.split_inclusive('\n')
                        .map(|line| DiffLine { kind: "minus".into(), text: line.to_string() })
                        .collect(),
                );
            } else {
                diff.kind = "binary".to_string();
            }
        }
        _ => {}
    }
    Ok(diff)
}

/// Сравнивает два дерева снимков и возвращает список отличий.
pub fn diff_trees(
    repo: &Repo,
    old_tree: &Tree,
    new_tree: &Tree,
) -> Result<Vec<FileDiff>> {
    let old_map: BTreeMap<&str, &crate::object::TreeEntry> = old_tree
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e))
        .collect();
    let new_map: BTreeMap<&str, &crate::object::TreeEntry> = new_tree
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e))
        .collect();

    let mut paths: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    paths.extend(old_map.keys().copied());
    paths.extend(new_map.keys().copied());

    let mut out = Vec::new();
    for path in paths {
        let o = old_map.get(path).map(|e| (e.hash.as_str(), e.size));
        let n = new_map.get(path).map(|e| (e.hash.as_str(), e.size));
        let changed = match (o, n) {
            (Some(a), Some(b)) => a.0 != b.0,
            (None, None) => false,
            _ => true,
        };
        if !changed {
            continue;
        }
        out.push(content_diff(repo, path, o, n)?);
    }
    Ok(out)
}

fn tree_of(repo: &Repo, hash: Option<&str>) -> Result<Tree> {
    match hash {
        Some(h) => {
            let snap = repo.snapshot(h)?;
            repo.tree_of(&snap)
        }
        None => {
            // HEAD или пустое дерево, если снимков ещё нет
            match repo.current_commit()? {
                Some(snap) => repo.tree_of(&snap),
                None => Ok(Tree::default()),
            }
        }
    }
}

/// Diff между двумя снимками (по хешам).
pub fn diff_snapshots(repo: &Repo, old_hash: &str, new_hash: &str) -> Result<Vec<FileDiff>> {
    let old_tree = tree_of(repo, Some(old_hash))?;
    let new_tree = tree_of(repo, Some(new_hash))?;
    diff_trees(repo, &old_tree, &new_tree)
}

/// Diff рабочей папки против снимка (`None` — последний снимок текущей ветки).
///
/// Содержательный diff строится для изменённых и добавленных файлов; для
/// удалённых показываются их старые строки. Файлы, отсутствующие в снимке
/// как «удалённые», в diff рабочей папки не имеют пары в снимке.
pub fn diff_working_vs_snapshot(repo: &Repo, hash: Option<&str>) -> Result<Vec<FileDiff>> {
    let tree = tree_of(repo, hash)?;
    let committed: BTreeMap<&str, &crate::object::TreeEntry> = tree
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e))
        .collect();

    let mut out = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for file in repo.list_files()? {
        seen.insert(file.name.clone());
        match committed.get(file.name.as_str()) {
            None => {
                // новый файл — показываем его содержимое как добавленное
                let bytes = match fs::read(repo.root().join(&file.name)) {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                };
                if file_kind(&file.name) == "csv" {
                    let table = match crate::merge::read_csv_table(&bytes, "рабочая копия") {
                        Ok(table) => table,
                        Err(_) => continue,
                    };
                    out.push(FileDiff {
                        path: file.name.clone(),
                        state: "added".into(),
                        kind: "csv".into(),
                        old_size: None,
                        new_size: Some(file.size),
                        lines: None,
                        csv: Some(CsvDiff {
                            header: table.0,
                            added: table.1,
                            removed: Vec::new(),
                            changed: Vec::new(),
                        }),
                    });
                } else if let Ok(text) = String::from_utf8(bytes) {
                    out.push(FileDiff {
                        path: file.name.clone(),
                        state: "added".into(),
                        kind: "text".into(),
                        old_size: None,
                        new_size: Some(file.size),
                        lines: Some(
                            text.split_inclusive('\n')
                                .map(|line| DiffLine { kind: "plus".into(), text: line.to_string() })
                                .collect(),
                        ),
                        csv: None,
                    });
                } else {
                    out.push(FileDiff {
                        path: file.name.clone(),
                        state: "added".into(),
                        kind: "binary".into(),
                        old_size: None,
                        new_size: Some(file.size),
                        lines: None,
                        csv: None,
                    });
                }
            }
            Some(entry) => {
                let current_hash = hash_file_content(&repo.root().join(&file.name))?;
                if current_hash != entry.hash {
                    // новое содержимое читаем с диска — блоба в хранилище может не быть
                    let new_bytes = fs::read(repo.root().join(&file.name)).ok();
                    out.push(diff_bytes(
                        &file.name,
                        Some((entry.hash.as_str(), entry.size)),
                        Some((current_hash.as_str(), file.size)),
                        repo.read_blob(&entry.hash).ok(),
                        new_bytes,
                    )?);
                }
            }
        }
    }

    // удалённые относительно снимка
    for (name, entry) in &committed {
        if !seen.contains(*name) {
            out.push(content_diff(repo, name, Some((entry.hash.as_str(), entry.size)), None)?);
        }
    }

    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::Repo;

    fn repo_in_tmp() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().expect("временная папка");
        let repo = Repo::init(dir.path()).expect("инициализация");
        (dir, repo)
    }

    #[test]
    fn статус_рабочей_папки_фиксирует_все_состояния() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("a.csv"), "k,v\n1,2\n").expect("файл");
        std::fs::write(repo.root().join("b.txt"), "hello").expect("файл");
        let _s1 = repo.save_snapshot("база", "Аня", &serde_json::Value::Null).expect("снимок");

        // пока ничего не меняли
        let status = working_status(&repo).expect("статус");
        assert!(status.iter().all(|s| s.state == "unmodified"));

        // изменяем один файл, добавляем второй, удаляем третий (новый)
        std::fs::write(repo.root().join("a.csv"), "k,v\n1,3\n").expect("правка");
        std::fs::write(repo.root().join("c.txt"), "новый").expect("новый файл");
        std::fs::remove_file(repo.root().join("b.txt")).expect("удаление");

        let status = working_status(&repo).expect("статус");
        let by_name = |name: &str| status.iter().find(|s| s.name == name).unwrap();
        assert_eq!(by_name("a.csv").state, "modified");
        assert_eq!(by_name("c.txt").state, "added");
        assert_eq!(by_name("b.txt").state, "deleted");
        assert_eq!(by_name("b.txt").previous_size, Some(5));
    }

    #[test]
    fn построчный_diff_помечает_строки() {
        let old = "a\nb\nc\n";
        let new = "a\nB\nc\nd\n";
        let lines = line_diff(old, new);
        let kinds: Vec<&str> = lines.iter().map(|l| l.kind.as_str()).collect();
        assert_eq!(kinds, vec!["equal", "minus", "plus", "equal", "plus"]);
        assert_eq!(lines[1].text, "b\n");
        assert_eq!(lines[2].text, "B\n");
    }

    #[test]
    fn diff_снимков_текст_и_csv() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("notes.txt"), "1\n2\n3\n").expect("текст");
        std::fs::write(repo.root().join("data.csv"), "gene_id,v\nG1,1\nG2,2\n").expect("csv");
        let s1 = repo.save_snapshot("первый", "Аня", &serde_json::Value::Null).expect("снимок");

        std::fs::write(repo.root().join("notes.txt"), "1\nдва\n3\n").expect("текст");
        std::fs::write(repo.root().join("data.csv"), "gene_id,v\nG1,1\nG2,3\nG3,4\n").expect("csv");
        let s2 = repo.save_snapshot("второй", "Аня", &serde_json::Value::Null).expect("снимок");

        let diffs = diff_snapshots(&repo, &s1.hash, &s2.hash).expect("diff");
        assert_eq!(diffs.len(), 2);

        let text = diffs.iter().find(|d| d.path == "notes.txt").unwrap();
        assert_eq!(text.kind, "text");
        let lines = text.lines.as_ref().unwrap();
        assert!(lines.iter().any(|l| l.kind == "minus" && l.text.trim() == "2"));
        assert!(lines.iter().any(|l| l.kind == "plus" && l.text.trim() == "два"));

        let csv = diffs.iter().find(|d| d.path == "data.csv").unwrap();
        assert_eq!(csv.kind, "csv");
        let csv_diff = csv.csv.as_ref().unwrap();
        assert_eq!(csv_diff.added.len(), 1, "добавлена строка G3");
        assert_eq!(csv_diff.changed.len(), 1, "изменена ячейка G2.v");
        assert_eq!(csv_diff.changed[0].key, "G2");
        assert_eq!(csv_diff.changed[0].column, "v");
        assert_eq!(csv_diff.changed[0].old, "2");
        assert_eq!(csv_diff.changed[0].new, "3");
    }

    #[test]
    fn diff_рабочей_папки_против_снимка() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("a.txt"), "старое").expect("файл");
        let s1 = repo.save_snapshot("база", "Аня", &serde_json::Value::Null).expect("снимок");

        std::fs::write(repo.root().join("a.txt"), "новое").expect("правка");
        std::fs::write(repo.root().join("b.txt"), "добавленный").expect("новый");

        let diffs = diff_working_vs_snapshot(&repo, Some(&s1.hash)).expect("diff");
        assert_eq!(diffs.len(), 2);
        assert!(diffs.iter().any(|d| d.path == "a.txt" && d.state == "modified"));
        assert!(diffs.iter().any(|d| d.path == "b.txt" && d.state == "added"));
    }

    #[test]
    fn csv_diff_удалённые_строки() {
        let old = b"gene_id,v\nG1,1\nG2,2\n";
        let new = b"gene_id,v\nG1,1\n";
        let diff = csv_diff(old, new).expect("diff");
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.removed[0][0], "G2");
        assert!(diff.added.is_empty() && diff.changed.is_empty());
    }
}

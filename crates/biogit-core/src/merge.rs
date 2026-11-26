//! Умное слияние ветвей-гипотез с разрешением конфликтов в интерфейсе.
//!
//! Двухфазная схема:
//! 1. [`propose_merge`] — вычисляет предложение слияния, ничего не записывая:
//!    успешно слитые файлы, структурированные конфликты (блоки текста,
//!    ячейки и строки CSV, двоичные файлы).
//! 2. [`apply_merge`] — применяет предложение с решениями пользователя
//!    ([`FileDecision`]) и создаёт снимок слияния.
//!
//! Три уровня содержимого:
//! 1. Файл целиком — сравнение по хешам.
//! 2. Текст — трёхстороннее построчное слияние.
//! 3. CSV — трёхстороннее слияние по ключевой колонке (настраиваемой,
//!    по умолчанию первая) с разбором через Polars.
//!
//! Старое поведение (`merge_branch`: чистое слияние выполняется, конфликт
//! отменяет его целиком) сохранено как обёртка над двухфазной схемой.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io::Write as _;

use serde::{Deserialize, Serialize};

use crate::error::{BioGitError, Result};
use crate::object::{Snapshot, Tree, TreeEntry};
use crate::snapshot::Repo;

/// Опции слияния.
#[derive(Debug, Clone, Default)]
pub struct MergeOptions {
    /// Ключевая колонка CSV-слияния. `None` — первая колонка.
    pub csv_key: Option<String>,
}

/// Отдельное неразрешимое расхождение (старый формат отчёта).
#[derive(Debug, Clone, Serialize)]
pub struct ConflictItem {
    pub file: String,
    pub reason: String,
}

/// Отчёт о слиянии ветвей-гипотез.
#[derive(Debug, Clone, Serialize)]
pub struct MergeReport {
    pub source: String,
    pub target: String,
    /// Файлы, в которые слияние принесло изменения (включая удалённые).
    pub merged: Vec<String>,
    /// Неразрешимые расхождения; при их наличии слияние отменено.
    pub conflicts: Vec<ConflictItem>,
    /// Новый снимок слияния (только если конфликтов не было).
    pub snapshot: Option<Snapshot>,
}

/// Сторона, выбираемая пользователем для конфликта.
pub const SIDE_OURS: &str = "ours";
pub const SIDE_THEIRS: &str = "theirs";

/// Структурированный конфликт внутри одного файла.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ConflictDetail {
    /// Конфликтный блок текста: обе ветки изменили пересекающиеся строки по-разному.
    #[serde(rename = "text_hunk")]
    TextHunk {
        index: usize,
        ours: Vec<String>,
        theirs: Vec<String>,
    },
    /// Конфликтная ячейка CSV: одна строка, одна колонка, разные значения.
    #[serde(rename = "csv_cell")]
    CsvCell {
        row_key: String,
        column: String,
        ours: String,
        theirs: String,
    },
    /// Конфликт целой строки CSV: удалена в одной ветке и изменена в другой,
    /// либо добавлена в обеих по-разному. `None` — версии строки нет у стороны.
    #[serde(rename = "csv_row")]
    CsvRow {
        row_key: String,
        ours: Option<Vec<String>>,
        theirs: Option<Vec<String>>,
    },
    /// Конфликт файла целиком: двоичный файл изменён с обеих сторон,
    /// структура колонок CSV различается и т.п.
    #[serde(rename = "file")]
    FileLevel {
        reason: String,
        #[serde(default = "default_true")]
        ours_exists: bool,
        #[serde(default = "default_true")]
        theirs_exists: bool,
    },
}

fn default_true() -> bool {
    true
}

impl ConflictDetail {
    /// Краткое человекочитаемое описание конфликта (для старого отчёта).
    fn short_reason(&self) -> String {
        match self {
            ConflictDetail::TextHunk { index, .. } => {
                format!("несовместимые правки текста (блок {index})")
            }
            ConflictDetail::CsvCell { row_key, column, .. } => {
                format!("строка «{row_key}», колонка «{column}»: изменена в обеих ветках по-разному")
            }
            ConflictDetail::CsvRow { row_key, ours, theirs } => {
                if ours.is_none() {
                    format!("строка «{row_key}»: удалена в целевой ветке и изменена в источнике")
                } else if theirs.is_none() {
                    format!("строка «{row_key}»: изменена в целевой ветке и удалена в источнике")
                } else {
                    format!("строка «{row_key}»: добавлена в обеих ветках с разным содержимым")
                }
            }
            ConflictDetail::FileLevel { reason, .. } => reason.clone(),
        }
    }
}

/// Файл в предложении слияния.
#[derive(Debug, Clone, Serialize)]
pub struct ProposedFile {
    pub path: String,
    /// "text" | "csv" | "binary"
    pub kind: String,
    /// "merged" — изменения будут применены; "conflict" — нужны решения.
    pub status: String,
    /// Откуда изменения: "both_changed" (нужно содержательное слияние),
    /// "theirs_only", "added_theirs", "deleted_theirs", "deleted_ours".
    pub variant: String,
    /// Слитый текст (kind = "text", статус "merged"): конфликтные блоки
    /// предварительно заполнены «нашими» строками — это предпросмотр,
    /// финальное содержимое собирается в apply с учётом решений.
    pub merged_text: Option<String>,
    /// Заголовок CSV (kind = "csv").
    pub csv_header: Option<Vec<String>>,
    /// Слитые строки CSV; конфликтные ячейки предварительно заполнены «нашими» значениями.
    pub csv_rows: Option<Vec<Vec<String>>>,
    pub conflicts: Vec<ConflictDetail>,
}

impl ProposedFile {
    fn clean(path: &str, kind: &str, variant: &str) -> ProposedFile {
        ProposedFile {
            path: path.to_string(),
            kind: kind.to_string(),
            status: "merged".to_string(),
            variant: variant.to_string(),
            merged_text: None,
            csv_header: None,
            csv_rows: None,
            conflicts: Vec::new(),
        }
    }

    fn conflicted(path: &str, kind: &str, conflicts: Vec<ConflictDetail>) -> ProposedFile {
        ProposedFile {
            path: path.to_string(),
            kind: kind.to_string(),
            status: "conflict".to_string(),
            variant: "both_changed".to_string(),
            merged_text: None,
            csv_header: None,
            csv_rows: None,
            conflicts,
        }
    }
}

/// Предложение слияния: результат `propose_merge` без записи на диск.
#[derive(Debug, Clone, Serialize)]
pub struct MergeProposal {
    pub source: String,
    pub target: String,
    pub base_hash: Option<String>,
    pub ours_hash: String,
    pub theirs_hash: String,
    /// Ключевая колонка CSV, использованная при расчёте.
    pub csv_key: Option<String>,
    /// Изменённые файлы (в порядке пути).
    pub files: Vec<ProposedFile>,
}

impl MergeProposal {
    /// Пути файлов с конфликтами.
    pub fn conflict_files(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|f| f.status == "conflict")
            .map(|f| f.path.clone())
            .collect()
    }

    /// Файлы без конфликтов, в которые будут внесены изменения.
    pub fn clean_changes(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|f| f.status == "merged")
            .map(|f| f.path.clone())
            .collect()
    }

    /// Есть ли конфликты, требующие решения.
    pub fn has_conflicts(&self) -> bool {
        self.files.iter().any(|f| f.status == "conflict")
    }
}

/// Решение пользователя по одному файлу.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDecision {
    pub path: String,
    /// "merged" — собрать слитое содержимое (по умолчанию); "ours"/"theirs" —
    /// взять версию стороны целиком (для двоичных файлов это единственный вариант).
    #[serde(default)]
    pub action: String,
    /// Текст: индекс конфликтного блока → "ours" | "theirs".
    #[serde(default)]
    pub hunks: BTreeMap<usize, String>,
    /// CSV-ячейки: "ключ\x1fколонка" → "ours" | "theirs".
    #[serde(default)]
    pub cells: BTreeMap<String, String>,
    /// CSV-строки: ключ → "ours" | "theirs".
    #[serde(default)]
    pub rows: BTreeMap<String, String>,
}

impl Default for FileDecision {
    fn default() -> Self {
        Self {
            path: String::new(),
            action: "merged".to_string(),
            hunks: BTreeMap::new(),
            cells: BTreeMap::new(),
            rows: BTreeMap::new(),
        }
    }
}

/// Выполняет слияние ветки `source` в текущую ветку репозитория (старое поведение).
///
/// Чистое слияние выполняется сразу; при наличии конфликтов слияние
/// отменяется целиком. Для разрешения конфликтов в интерфейсе используйте
/// [`propose_merge`] + [`apply_merge`].
pub fn merge_branch(repo: &Repo, source: &str) -> Result<MergeReport> {
    merge_branch_with_options(repo, source, &MergeOptions::default())
}

/// То же, что [`merge_branch`], но с настраиваемым ключом CSV-слияния.
pub fn merge_branch_with_options(
    repo: &Repo,
    source: &str,
    options: &MergeOptions,
) -> Result<MergeReport> {
    let proposal = propose_merge(repo, source, options)?;
    if !proposal.has_conflicts() {
        return apply_merge(repo, source, options, &[]);
    }
    // Старое поведение: отмена целиком, отчёт с причинами
    let mut conflicts = Vec::new();
    let target_branch = proposal.target.clone();
    let clean_changes = proposal.clean_changes();
    for file in &proposal.files {
        if file.status == "conflict" {
            let reason = file
                .conflicts
                .first()
                .map(ConflictDetail::short_reason)
                .unwrap_or_else(|| "неразрешимое расхождение".to_string());
            conflicts.push(ConflictItem {
                file: file.path.clone(),
                reason,
            });
        }
    }
    Ok(MergeReport {
        source: source.to_string(),
        target: target_branch,
        merged: clean_changes,
        conflicts,
        snapshot: None,
    })
}

/// Фаза 1: вычисляет предложение слияния, ничего не изменяя.
pub fn propose_merge(
    repo: &Repo,
    source: &str,
    options: &MergeOptions,
) -> Result<MergeProposal> {
    crate::snapshot::validate_branch_name(source)?;
    let target_branch = repo.head()?;
    let ours = repo.current_commit()?.ok_or_else(|| {
        BioGitError::InvalidState(format!("на ветке «{target_branch}» ещё нет снимков"))
    })?;
    let theirs = repo.snapshot_of_branch(source)?;

    if theirs.hash == ours.hash {
        // слияние с самим собой — изменений нет
        return Ok(MergeProposal {
            source: source.to_string(),
            target: target_branch,
            base_hash: Some(ours.hash.clone()),
            ours_hash: ours.hash.clone(),
            theirs_hash: theirs.hash.clone(),
            csv_key: options.csv_key.clone(),
            files: Vec::new(),
        });
    }

    let base = merge_base(repo, &ours.hash, &theirs.hash)?;
    let base_tree = match &base {
        Some(b) => repo.tree_of(b)?,
        None => Tree::default(),
    };
    let ours_tree = repo.tree_of(&ours)?;
    let theirs_tree = repo.tree_of(&theirs)?;

    fn index(t: &Tree) -> HashMap<&str, &TreeEntry> {
        t.entries.iter().map(|e| (e.name.as_str(), e)).collect()
    }
    let base_map = index(&base_tree);
    let ours_map = index(&ours_tree);
    let theirs_map = index(&theirs_tree);

    let mut paths: BTreeSet<&str> = BTreeSet::new();
    paths.extend(base_map.keys());
    paths.extend(ours_map.keys());
    paths.extend(theirs_map.keys());

    let mut files: Vec<ProposedFile> = Vec::new();

    for path in paths {
        let b = base_map.get(path).copied();
        let o = ours_map.get(path).copied();
        let t = theirs_map.get(path).copied();

        match (o, t) {
            (Some(oe), Some(te)) if oe.hash == te.hash => {} // одинаково
            (Some(_s2), Some(te)) if same_content(b, te) => {} // меняли только у нас
            (Some(oe), Some(te)) if same_content(b, oe) => {
                // меняли только в источнике — берём их версию
                let kind = file_kind(path);
                files.push(ProposedFile::clean(path, kind, "theirs_only"));
            }
            (Some(oe), Some(te)) => {
                // изменён с обеих сторон — содержательное слияние
                files.push(propose_content(repo, path, b, oe, te, options)?);
            }
            (Some(oe), None) => {
                if b.is_none() {
                    // добавлен только у нас — остаётся без изменений
                } else if same_content(b, oe) {
                    files.push(ProposedFile::clean(path, "text", "deleted_theirs"));
                } else {
                    files.push(ProposedFile::conflicted(
                        path,
                        "binary",
                        vec![ConflictDetail::FileLevel {
                            reason: "файл изменён в целевой ветке и удалён в источнике"
                                .to_string(),
                            ours_exists: true,
                            theirs_exists: false,
                        }],
                    ));
                }
            }
            (None, Some(te)) => {
                if b.is_none() {
                    let kind = file_kind(path);
                    files.push(ProposedFile::clean(path, kind, "added_theirs"));
                } else if same_content(b, te) {
                    files.push(ProposedFile::clean(path, "text", "deleted_ours"));
                } else {
                    files.push(ProposedFile::conflicted(
                        path,
                        "binary",
                        vec![ConflictDetail::FileLevel {
                            reason: "файл удалён в целевой ветке и изменён в источнике"
                                .to_string(),
                            ours_exists: false,
                            theirs_exists: true,
                        }],
                    ));
                }
            }
            (None, None) => {} // удалён в обеих ветках
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(MergeProposal {
        source: source.to_string(),
        target: target_branch,
        base_hash: base.map(|b| b.hash),
        ours_hash: ours.hash,
        theirs_hash: theirs.hash,
        csv_key: options.csv_key.clone(),
        files,
    })
}

fn file_kind(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if matches!(ext.as_str(), "csv" | "tsv") {
        "csv"
    } else {
        "text"
    }
}

/// Содержательное предложение для файла, изменённого в обеих ветках.
fn propose_content(
    repo: &Repo,
    path: &str,
    base: Option<&TreeEntry>,
    ours: &TreeEntry,
    theirs: &TreeEntry,
    options: &MergeOptions,
) -> Result<ProposedFile> {
    let base_bytes = match base {
        Some(e) => Some(repo.read_blob(&e.hash)?),
        None => None,
    };
    let ours_bytes = repo.read_blob(&ours.hash)?;
    let theirs_bytes = repo.read_blob(&theirs.hash)?;

    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let is_csv = matches!(ext.as_str(), "csv" | "tsv");

    if is_csv {
        let outcome = merge_csv_with_details(
            base_bytes.as_deref(),
            &ours_bytes,
            &theirs_bytes,
            options.csv_key.as_deref(),
        );
        return match outcome {
            Err(reason) => {
                // неразрешимая структура (например, различие заголовков) — конфликт файла
                Ok(ProposedFile::conflicted(
                    path,
                    "csv",
                    vec![ConflictDetail::FileLevel {
                        reason,
                        ours_exists: true,
                        theirs_exists: true,
                    }],
                ))
            }
            Ok(outcome) => {
                let status = if outcome.conflicts.is_empty() { "merged" } else { "conflict" };
                Ok(ProposedFile {
                    path: path.to_string(),
                    kind: "csv".to_string(),
                    status: status.to_string(),
                    variant: "both_changed".to_string(),
                    merged_text: None,
                    csv_header: Some(outcome.header),
                    csv_rows: Some(outcome.rows),
                    conflicts: outcome.conflicts,
                })
            }
        };
    }

    let ours_text = std::str::from_utf8(&ours_bytes).ok().map(str::as_bytes);
    let theirs_text = std::str::from_utf8(&theirs_bytes).ok().map(str::as_bytes);
    let base_text = base_bytes
        .as_deref()
        .and_then(|b| std::str::from_utf8(b).ok())
        .map(str::as_bytes);

    match (ours_text, theirs_text) {
        (Some(o), Some(t)) => {
            let segments = merge_text_segments(base_text, o, t);
            let mut conflicts = Vec::new();
            let mut index = 0usize;
            let mut merged = Vec::new();
            for segment in &segments {
                match segment {
                    TextSegment::Lines(lines) => merged.extend(lines.iter().cloned()),
                    TextSegment::Conflict { ours, theirs } => {
                        conflicts.push(ConflictDetail::TextHunk {
                            index,
                            ours: ours.clone(),
                            theirs: theirs.clone(),
                        });
                        // предпросмотр: конфликтный блок заполнен «нашими» строками
                        merged.extend(ours.iter().cloned());
                        index += 1;
                    }
                }
            }
            let status = if conflicts.is_empty() { "merged" } else { "conflict" };
            Ok(ProposedFile {
                path: path.to_string(),
                kind: "text".to_string(),
                status: status.to_string(),
                variant: "both_changed".to_string(),
                merged_text: Some(merged.concat()),
                csv_header: None,
                csv_rows: None,
                conflicts,
            })
        }
        _ => Ok(ProposedFile::conflicted(
            path,
            "binary",
            vec![ConflictDetail::FileLevel {
                reason: "двоичный файл изменён в обеих ветках".to_string(),
                ours_exists: true,
                theirs_exists: true,
            }],
        )),
    }
}

// ---------------------------------------------------------------------------
// Фаза 2: применение предложения с решениями
// ---------------------------------------------------------------------------

/// Фаза 2: применяет предложение слияния с решениями пользователя и создаёт
/// снимок слияния. Отсутствующие решения трактуются как «наша версия».
pub fn apply_merge(
    repo: &Repo,
    source: &str,
    options: &MergeOptions,
    decisions: &[FileDecision],
) -> Result<MergeReport> {
    let proposal = propose_merge(repo, source, options)?;
    apply_proposal(repo, proposal, decisions)
}

/// Применяет готовое предложение (предложение пересчитывается детерминированно
/// из репозитория, поэтому его можно безопасно передать от интерфейса).
pub fn apply_proposal(
    repo: &Repo,
    proposal: MergeProposal,
    decisions: &[FileDecision],
) -> Result<MergeReport> {
    if proposal.files.is_empty() && proposal.ours_hash == proposal.theirs_hash {
        let ours = repo.snapshot(&proposal.ours_hash)?;
        return Ok(MergeReport {
            source: proposal.source,
            target: proposal.target,
            merged: Vec::new(),
            conflicts: Vec::new(),
            snapshot: Some(ours),
        });
    }

    let decision_map: HashMap<&str, &FileDecision> = decisions
        .iter()
        .map(|d| (d.path.as_str(), d))
        .collect();

    let base_tree = match &proposal.base_hash {
        Some(h) => repo.tree_of(&repo.snapshot(h)?)?,
        None => Tree::default(),
    };
    let ours_snap = repo.snapshot(&proposal.ours_hash)?;
    let theirs_snap = repo.snapshot(&proposal.theirs_hash)?;
    let ours_tree = repo.tree_of(&ours_snap)?;
    let theirs_tree = repo.tree_of(&theirs_snap)?;

    fn index(t: &Tree) -> HashMap<&str, &TreeEntry> {
        t.entries.iter().map(|e| (e.name.as_str(), e)).collect()
    }
    let base_map = index(&base_tree);
    let ours_map = index(&ours_tree);
    let theirs_map = index(&theirs_tree);

    let mut paths: BTreeSet<&str> = BTreeSet::new();
    paths.extend(base_map.keys());
    paths.extend(ours_map.keys());
    paths.extend(theirs_map.keys());

    let proposed_by_path: HashMap<&str, &ProposedFile> = proposal
        .files
        .iter()
        .map(|f| (f.path.as_str(), f))
        .collect();

    let mut result_entries: Vec<TreeEntry> = Vec::new();

    for path in paths {
        let b = base_map.get(path).copied();
        let o = ours_map.get(path).copied();
        let t = theirs_map.get(path).copied();

        let final_entry: Option<TreeEntry> = match proposed_by_path.get(path).copied() {
            Some(file) => resolve_proposed_file(
                repo,
                file,
                decision_map.get(path).copied(),
                proposal.csv_key.as_deref(),
                b,
                o,
                t,
            )?,
            None => match (o, t) {
                (Some(oe), _) => Some(oe.clone()), // не менялся / меняли только у нас
                (None, Some(te)) if b.is_none() => Some(te.clone()), // добавлен в источнике
                (None, Some(_)) => None,             // удалён у нас, в источнике не меняли
                (None, None) => None,
            },
        };

        if let Some(entry) = final_entry {
            result_entries.push(entry);
        }
    }

    result_entries.sort_by(|a, b| a.name.cmp(&b.name));

    // Список изменений: итог отличается от нашей версии
    let mut merged = Vec::new();
    let ours_index: HashMap<&str, &TreeEntry> = ours_tree
        .entries
        .iter()
        .map(|e| (e.name.as_str(), e))
        .collect();
    let final_names: HashSet<String> = result_entries
        .iter()
        .map(|e| e.name.clone())
        .collect();
    for entry in &result_entries {
        match ours_index.get(entry.name.as_str()) {
            None => merged.push(entry.name.clone()),
            Some(old) => {
                if old.hash != entry.hash {
                    merged.push(entry.name.clone());
                }
            }
        }
    }
    for name in ours_index.keys() {
        if !final_names.contains(*name) {
            merged.push(format!("{name} (удалён)"));
        }
    }

    let tree = Tree {
        entries: result_entries,
    };
    let tree_hash = repo.store_tree(&tree)?;
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let message = format!("Слияние ветви гипотезы «{}»", proposal.source);
    let parent = vec![proposal.ours_hash.clone(), proposal.theirs_hash.clone()];
    let files_count = tree.entries.len();
    let hash = Snapshot::compute_hash(
        &timestamp,
        &ours_snap.author,
        &message,
        &parent,
        &proposal.target,
        &tree_hash,
        &ours_snap.parameters,
        files_count,
    );
    let snapshot = Snapshot {
        hash: hash.clone(),
        timestamp,
        author: ours_snap.author.clone(),
        message,
        parent,
        branch: proposal.target.clone(),
        tree: tree_hash,
        parameters: ours_snap.parameters.clone(),
        files_count,
    };
    repo.write_commit_object(&snapshot)?;
    repo.set_branch_ref(&proposal.target, &hash)?;
    repo.restore(&hash)?;

    // Синхронизация рабочей папки: файлы, известные репозиторию (в base/ours/theirs),
    // но отсутствующие в итоговом дереве, удаляются с диска (как это делает git).
    // Неотслеживаемые файлы не трогаются.
    let mut tracked: HashSet<&str> = HashSet::new();
    for tree in [&base_tree, &ours_tree, &theirs_tree] {
        tracked.extend(tree.entries.iter().map(|e| e.name.as_str()));
    }
    for name in tracked {
        if !final_names.contains(name) {
            let path = repo.root().join(name);
            if path.is_file() {
                std::fs::remove_file(path)?;
            }
        }
    }

    Ok(MergeReport {
        source: proposal.source,
        target: proposal.target,
        merged,
        conflicts: Vec::new(),
        snapshot: Some(snapshot),
    })
}

/// Сериализация CSV (заголовок + строки).
fn csv_bytes(header: &[String], rows: &[Vec<String>]) -> Vec<u8> {
    let mut out = Vec::new();
    push_row(header, &mut out);
    for row in rows {
        push_row(row, &mut out);
    }
    out
}

/// Собирает итоговое содержимое предложенного файла по решениям пользователя.
/// Возвращает `None`, если файл в итоге отсутствует. Содержимое конфликтного
/// файла пересчитывается из блобов сторон (детерминированно) с применением решений.
#[allow(clippy::too_many_arguments)]
fn resolve_proposed_file(
    repo: &Repo,
    file: &ProposedFile,
    decision: Option<&FileDecision>,
    csv_key: Option<&str>,
    base: Option<&TreeEntry>,
    ours: Option<&TreeEntry>,
    theirs: Option<&TreeEntry>,
) -> Result<Option<TreeEntry>> {
    let action = decision
        .and_then(|d| (!d.action.is_empty()).then_some(d.action.as_str()))
        .unwrap_or("merged");

    match action {
        "ours" => return Ok(ours.cloned()),
        "theirs" => return Ok(theirs.cloned()),
        _ => {}
    }

    // Односторонние изменения — файл целиком берётся из своей стороны
    match file.variant.as_str() {
        "theirs_only" | "added_theirs" => return Ok(theirs.cloned()),
        "deleted_theirs" | "deleted_ours" => return Ok(None),
        _ => {} // both_changed — содержательное слияние
    }

    if file.status == "merged" {
        let content: Vec<u8> = match file.kind.as_str() {
            "csv" => {
                let header = file.csv_header.clone().unwrap_or_default();
                let rows = file.csv_rows.clone().unwrap_or_default();
                csv_bytes(&header, &rows)
            }
            _ => file.merged_text.clone().unwrap_or_default().into_bytes(),
        };
        let hash = repo.store().put_bytes(&content)?;
        return Ok(Some(TreeEntry {
            name: file.path.clone(),
            hash,
            size: content.len() as u64,
        }));
    }

    // Статус "conflict" — пересчитываем слияние из блобов сторон
    // и применяем решения пользователя к конфликтным элементам.
    let empty = Vec::new();
    let base_bytes = match base {
        Some(e) => Some(repo.read_blob(&e.hash)?),
        None => None,
    };
    let ours_bytes = match ours {
        Some(e) => repo.read_blob(&e.hash)?,
        None => empty.clone(),
    };
    let theirs_bytes = match theirs {
        Some(e) => repo.read_blob(&e.hash)?,
        None => empty,
    };

    match file.kind.as_str() {
        "csv" => {
            // Файловый конфликт (различие структуры колонок) — без явного
            // решения берём «нашую» версию целиком.
            if file
                .conflicts
                .iter()
                .any(|c| matches!(c, ConflictDetail::FileLevel { .. }))
            {
                return Ok(ours.cloned());
            }
            let outcome = merge_csv_with_details(
                base_bytes.as_deref(),
                &ours_bytes,
                &theirs_bytes,
                csv_key,
            )
            .map_err(BioGitError::Merge)?;

            let header = &outcome.header;
            let mut rows: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for row in &outcome.rows {
                if let Some(key) = row.first() {
                    rows.insert(key.clone(), row.clone());
                }
            }

            for conflict in &outcome.conflicts {
                match conflict {
                    ConflictDetail::CsvCell { row_key, column, ours, theirs } => {
                        let side = decision
                            .and_then(|d| d.cells.get(&cell_key(row_key, column)))
                            .map(String::as_str)
                            .unwrap_or(SIDE_OURS);
                        let value = if side == SIDE_THEIRS { theirs } else { ours };
                        if let Some(row) = rows.get_mut(row_key) {
                            if let Some(c) = header.iter().position(|h| h == column) {
                                row[c] = value.clone();
                            }
                        }
                    }
                    ConflictDetail::CsvRow { row_key, ours, theirs } => {
                        let side = decision
                            .and_then(|d| d.rows.get(row_key))
                            .map(String::as_str)
                            .unwrap_or(SIDE_OURS);
                        let value = if side == SIDE_THEIRS { theirs } else { ours };
                        match value {
                            Some(row) => {
                                rows.insert(row_key.clone(), row.clone());
                            }
                            None => {
                                rows.remove(row_key);
                            }
                        }
                    }
                    _ => {}
                }
            }

            let content = csv_bytes(header, &rows.into_values().collect::<Vec<_>>());
            let hash = repo.store().put_bytes(&content)?;
            Ok(Some(TreeEntry {
                name: file.path.clone(),
                hash,
                size: content.len() as u64,
            }))
        }
        "text" => {
            let segments =
                merge_text_segments(base_bytes.as_deref(), &ours_bytes, &theirs_bytes);
            let mut out: Vec<String> = Vec::new();
            let mut index = 0usize;
            for segment in segments {
                match segment {
                    TextSegment::Lines(lines) => out.extend(lines),
                    TextSegment::Conflict { ours, theirs } => {
                        let side = decision
                            .and_then(|d| d.hunks.get(&index))
                            .map(String::as_str)
                            .unwrap_or(SIDE_OURS);
                        if side == SIDE_THEIRS {
                            out.extend(theirs);
                        } else {
                            out.extend(ours);
                        }
                        index += 1;
                    }
                }
            }
            let content = out.concat().into_bytes();
            let hash = repo.store().put_bytes(&content)?;
            Ok(Some(TreeEntry {
                name: file.path.clone(),
                hash,
                size: content.len() as u64,
            }))
        }
        // двоичный конфликт без явного решения — «наша» версия
        _ => Ok(ours.cloned()),
    }
}

/// Ключ ячейки в решениях: `ключ\x1fколонка`.
fn cell_key(row_key: &str, column: &str) -> String {
    format!("{row_key}\u{1f}{column}")
}
// ---------------------------------------------------------------------------
// Текстовое слияние (хунковый трёхсторонний алгоритм, как в git)
// ---------------------------------------------------------------------------

/// Сегмент результата текстового слияния: строки или конфликтный блок.
#[derive(Debug, Clone)]
pub(crate) enum TextSegment {
    Lines(Vec<String>),
    Conflict { ours: Vec<String>, theirs: Vec<String> },
}

/// Хунок изменения: base[z0..z1) заменено на other[s0..s1).
type Hunk = (usize, usize, usize, usize);

/// Трёхстороннее построчное слияние текста с сохранением конфликтных блоков.
///
/// Строим два набора изменённых областей: base→наши и base→их.
/// Непересекающиеся области применяются независимо (правки соседних строк
/// объединяются), пересекающиеся — сравниваются по результату: одинаковые
/// применяются один раз, разные становятся конфликтным сегментом.
pub(crate) fn merge_text_segments(
    base: Option<&[u8]>,
    ours: &[u8],
    theirs: &[u8],
) -> Vec<TextSegment> {
    let split = |data: &[u8]| -> Vec<String> {
        std::str::from_utf8(data)
            .unwrap_or("")
            .split_inclusive('\n')
            .map(str::to_string)
            .collect()
    };
    let b = base.map(split).unwrap_or_default();
    let o = split(ours);
    let t = split(theirs);

    let ours_hunks = changed_regions(&b, &o);
    let theirs_hunks = changed_regions(&b, &t);

    let mut out: Vec<TextSegment> = Vec::new();
    let mut p = 0usize;
    let (mut ia, mut it) = (0usize, 0usize);

    while ia < ours_hunks.len() || it < theirs_hunks.len() {
        let start = match (ours_hunks.get(ia), theirs_hunks.get(it)) {
            (Some(a), Some(h2)) => a.0.min(h2.0),
            (Some(a), None) => a.0,
            (None, Some(h2)) => h2.0,
            (None, None) => break,
        };
        if start > p {
            out.push(TextSegment::Lines(b[p..start].to_vec()));
            p = start;
        }

        // Группа пересекающихся хунков с обеих сторон
        let mut end = p;
        let (a_from, t_from) = (ia, it);
        if ia < ours_hunks.len() && ours_hunks[ia].0 == p {
            end = end.max(ours_hunks[ia].1);
            ia += 1;
        }
        if it < theirs_hunks.len() && theirs_hunks[it].0 == p {
            end = end.max(theirs_hunks[it].1);
            it += 1;
        }
        loop {
            let mut grew = false;
            while ia < ours_hunks.len() && ours_hunks[ia].0 < end {
                end = end.max(ours_hunks[ia].1);
                ia += 1;
                grew = true;
            }
            while it < theirs_hunks.len() && theirs_hunks[it].0 < end {
                end = end.max(theirs_hunks[it].1);
                it += 1;
                grew = true;
            }
            if !grew {
                break;
            }
        }

        let our_text = apply_hunks(&b[p..end], &o, &ours_hunks[a_from..ia], p);
        let their_text = apply_hunks(&b[p..end], &t, &theirs_hunks[t_from..it], p);
        if a_from == ia {
            out.push(TextSegment::Lines(their_text)); // в области меняли только «их»
        } else if t_from == it {
            out.push(TextSegment::Lines(our_text)); // меняли только «наши»
        } else if our_text == their_text {
            out.push(TextSegment::Lines(our_text)); // обе ветки изменили одинаково
        } else {
            out.push(TextSegment::Conflict {
                ours: our_text,
                theirs: their_text,
            });
        }
        p = end;
    }
    if p < b.len() {
        out.push(TextSegment::Lines(b[p..].to_vec()));
    }
    out
}

/// Старый интерфейс текстового слияния: Ok(байты) при чистом слиянии,
/// Err(причина) при конфликте.
pub(crate) type ContentMergeOutcome = std::result::Result<Vec<u8>, String>;

pub(crate) fn merge_text_three_way(
    base: Option<&[u8]>,
    ours: &[u8],
    theirs: &[u8],
) -> Result<ContentMergeOutcome> {
    let segments = merge_text_segments(base, ours, theirs);
    let mut conflicts = 0usize;
    let mut out: Vec<String> = Vec::new();
    for segment in segments {
        match segment {
            TextSegment::Lines(lines) => out.extend(lines),
            TextSegment::Conflict { .. } => conflicts += 1,
        }
    }
    if conflicts == 0 {
        Ok(Ok(out.concat().into_bytes()))
    } else {
        Ok(Err(format!(
            "несовместимые правки текста: {conflicts} конфликтных блоков"
        )))
    }
}

/// Непрерывные области отличий base от other (по LCS-совпадениям).
fn changed_regions(base: &[String], other: &[String]) -> Vec<Hunk> {
    let matches = lcs_matches(base, other);
    let mut hunks = Vec::new();
    let mut bi = 0usize;
    let mut oi = 0usize;
    for &(i, j) in &matches {
        if i > bi || j > oi {
            hunks.push((bi, i, oi, j));
        }
        bi = i + 1;
        oi = j + 1;
    }
    if bi < base.len() || oi < other.len() {
        hunks.push((bi, base.len(), oi, other.len()));
    }
    hunks
}

/// Применяет хунки к срезу base_slice (глобальные индексы base начинаются с offset).
fn apply_hunks(
    base_slice: &[String],
    other: &[String],
    hunks: &[Hunk],
    offset: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = offset;
    for &(z0, z1, s0, s1) in hunks {
        if z0 > pos {
            out.extend_from_slice(&base_slice[pos - offset..z0 - offset]);
        }
        out.extend_from_slice(&other[s0..s1]);
        pos = z1;
    }
    if pos < offset + base_slice.len() {
        out.extend_from_slice(&base_slice[pos - offset..]);
    }
    out
}

/// Парные индексы совпадающих строк (одна из монотонных LCS-подпоследовательностей).
pub(crate) fn lcs_matches(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let n = a.len();
    let m = b.len();
    if n == 0 || m == 0 {
        return Vec::new();
    }
    // Защита от огромных файлов: DP-таблица не более ~4 МБ
    if n.saturating_mul(m) > 1_000_000 {
        return Vec::new();
    }
    let width = m + 1;
    let mut dp = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * width + j] = if a[i] == b[j] {
                dp[(i + 1) * width + j + 1] + 1
            } else {
                dp[(i + 1) * width + j].max(dp[i * width + j + 1])
            };
        }
    }

    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if dp[(i + 1) * width + j] >= dp[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Слияние CSV по ключевой колонке
// ---------------------------------------------------------------------------

/// Таблица CSV: заголовок + строки (значения — текст).
pub(crate) type CsvTable = (Vec<String>, Vec<Vec<String>>);

/// Результат слияния CSV со структурированными конфликтами.
pub(crate) struct CsvMergeOutcome {
    pub header: Vec<String>,
    /// Слитые строки; конфликтные ячейки заполнены «нашими» значениями.
    pub rows: Vec<Vec<String>>,
    pub conflicts: Vec<ConflictDetail>,
}

/// Трёхстороннее слияние CSV по ключевой колонке (по умолчанию первая).
pub(crate) fn merge_csv_with_details(
    base: Option<&[u8]>,
    ours: &[u8],
    theirs: &[u8],
    key_column: Option<&str>,
) -> std::result::Result<CsvMergeOutcome, String> {
    let ours_table = read_csv_table(ours, "рабочая копия").map_err(|e| e.to_string())?;
    let theirs_table = read_csv_table(theirs, "ветвь-источник").map_err(|e| e.to_string())?;
    let base_table = match base {
        Some(bytes) => Some(read_csv_table(bytes, "базовый снимок").map_err(|e| e.to_string())?),
        None => None,
    };
    merge_tables_with_details(base_table.as_ref(), &ours_table, &theirs_table, key_column)
}

/// Старый интерфейс CSV-слияния.
pub(crate) fn merge_csv_three_way(
    base: Option<&[u8]>,
    ours: &[u8],
    theirs: &[u8],
) -> Result<ContentMergeOutcome> {
    match merge_csv_with_details(base, ours, theirs, None) {
        Err(reason) => Ok(Err(reason)),
        Ok(outcome) => {
            if !outcome.conflicts.is_empty() {
                // старое поведение: любой конфликт отменяет слияние целиком
                let reason = outcome
                    .conflicts
                    .first()
                    .map(ConflictDetail::short_reason)
                    .unwrap_or_else(|| "конфликт CSV".to_string());
                return Ok(Err(reason));
            }
            let mut out = Vec::new();
            push_row(&outcome.header, &mut out);
            for row in &outcome.rows {
                push_row(row, &mut out);
            }
            Ok(Ok(out))
        }
    }
}

pub(crate) fn read_csv_table(bytes: &[u8], label: &str) -> Result<CsvTable> {
    use polars::prelude::*;

    // Polars читает из файла — пишем байты во временный файл
    let mut tmp = tempfile::NamedTempFile::new()?;
    tmp.write_all(bytes)?;
    let path = tmp.path().to_path_buf();

    let df = CsvReadOptions::default()
        .with_has_header(true)
        .try_into_reader_with_file_path(Some(path))?
        .finish()?;

    let header: Vec<String> = df.get_column_names().iter().map(|s| s.to_string()).collect();
    if header.is_empty() {
        return Err(BioGitError::Csv(format!("{label}: пустой CSV")));
    }
    let columns = df.get_columns();
    let mut rows = Vec::with_capacity(df.height());
    for i in 0..df.height() {
        let mut row = Vec::with_capacity(header.len());
        for col in columns {
            let series = col.as_materialized_series();
            let text = if series.dtype().is_string() {
                match series.str().map_err(BioGitError::from)?.get(i) {
                    Some(value) => value.to_string(),
                    None => String::new(),
                }
            } else {
                let value = series.get(i).map_err(BioGitError::from)?;
                format!("{value}")
            };
            row.push(text);
        }
        rows.push(row);
    }
    Ok((header, rows))
}

fn merge_tables_with_details(
    base: Option<&CsvTable>,
    ours: &CsvTable,
    theirs: &CsvTable,
    key_column: Option<&str>,
) -> std::result::Result<CsvMergeOutcome, String> {
    let (header, ours_rows) = ours;
    let (_, theirs_rows) = theirs;

    if header.is_empty() {
        return Err("CSV без колонок".to_string());
    }
    if ours.0 != theirs.0 {
        return Err("структура колонок изменена в обеих ветках по-разному".to_string());
    }

    let key_idx = match key_column {
        None => 0usize,
        Some(name) => header
            .iter()
            .position(|h| h == name)
            .ok_or_else(|| format!("ключевая колонка «{name}» не найдена в CSV"))?,
    };
    let key_name = &header[key_idx];

    let to_map = |rows: &[Vec<String>], label: &str| -> std::result::Result<HashMap<String, Vec<String>>, String> {
        let mut map = HashMap::new();
        for row in rows {
            if row.len() != header.len() {
                return Err(format!("{label}: строка с {} полями вместо {}", row.len(), header.len()));
            }
            let key = row[key_idx].clone();
            if key.is_empty() {
                return Err(format!("{label}: пустое значение ключа в колонке «{key_name}»"));
            }
            if map.insert(key, row.clone()).is_some() {
                return Err(format!("{label}: дубликат ключа в колонке «{key_name}»"));
            }
        }
        Ok(map)
    };

    let om = to_map(ours_rows, "рабочая копия")?;
    let tm = to_map(theirs_rows, "ветвь-источник")?;
    let bm: HashMap<String, Vec<String>> = match base {
        Some((_, rows)) => to_map(rows, "базовый снимок")?,
        None => HashMap::new(),
    };

    let mut keys: BTreeSet<String> = BTreeSet::new();
    keys.extend(om.keys().cloned());
    keys.extend(tm.keys().cloned());
    keys.extend(bm.keys().cloned());

    let mut rows_out = Vec::new();
    let mut conflicts = Vec::new();

    for key in keys {
        let b = bm.get(&key);
        let o = om.get(&key);
        let t = tm.get(&key);
        match (o, t) {
            (Some(x), Some(y)) if x == y => rows_out.push(x.clone()),
            (Some(x), Some(y)) => {
                if b == Some(y) {
                    rows_out.push(x.clone());
                } else if b == Some(x) {
                    rows_out.push(y.clone());
                } else {
                    // обе ветки изменили строку — поячеечное слияние
                    match b {
                        Some(base_row)
                            if base_row.len() == x.len() && x.len() == y.len() =>
                        {
                            let mut row = Vec::with_capacity(x.len());
                            for c in 0..x.len() {
                                if x[c] == y[c] {
                                    row.push(x[c].clone());
                                } else if base_row[c] == x[c] {
                                    row.push(y[c].clone());
                                } else if base_row[c] == y[c] {
                                    row.push(x[c].clone());
                                } else {
                                    let col = header.get(c).cloned().unwrap_or_else(|| "?".to_string());
                                    conflicts.push(ConflictDetail::CsvCell {
                                        row_key: key.clone(),
                                        column: col,
                                        ours: x[c].clone(),
                                        theirs: y[c].clone(),
                                    });
                                    row.push(x[c].clone()); // предзаполняем «нашим»
                                }
                            }
                            rows_out.push(row);
                        }
                        _ => {
                            conflicts.push(ConflictDetail::CsvRow {
                                row_key: key.clone(),
                                ours: Some(x.clone()),
                                theirs: Some(y.clone()),
                            });
                            rows_out.push(x.clone()); // предзаполняем «нашей»
                        }
                    }
                }
            }
            (Some(x), None) => {
                if b.is_none() {
                    rows_out.push(x.clone());
                } else if b == Some(x) {
                    // удалено только в источнике — строка не включается
                } else {
                    // изменена у нас, удалена в источнике — конфликт,
                    // строка не предзаполняется: решение «наша» вернёт её при apply
                    conflicts.push(ConflictDetail::CsvRow {
                        row_key: key.clone(),
                        ours: Some(x.clone()),
                        theirs: None,
                    });
                }
            }
            (None, Some(y)) => {
                if b.is_none() {
                    rows_out.push(y.clone());
                } else if b == Some(y) {
                    // удалено у нас — строка не включается
                } else {
                    conflicts.push(ConflictDetail::CsvRow {
                        row_key: key.clone(),
                        ours: None,
                        theirs: Some(y.clone()),
                    });
                }
            }
            (None, None) => {} // удалено в обеих ветках
        }
    }

    Ok(CsvMergeOutcome {
        header: header.clone(),
        rows: rows_out,
        conflicts,
    })
}

fn csv_escape(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn push_row(fields: &[String], out: &mut Vec<u8>) {
    let mut line = String::new();
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            line.push(',');
        }
        line.push_str(&csv_escape(field));
    }
    line.push('\n');
    out.extend_from_slice(line.as_bytes());
}

/// Общий предок двух снимков: все предки `a`, затем поиск в ширину от `b`.
fn merge_base(repo: &Repo, a: &str, b: &str) -> Result<Option<Snapshot>> {
    let mut ancestors: HashSet<String> = HashSet::new();
    let mut stack = vec![a.to_string()];
    while let Some(h) = stack.pop() {
        if ancestors.insert(h.clone()) {
            let snap = repo.snapshot(&h)?;
            stack.extend(snap.parent.iter().cloned());
        }
    }

    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(b.to_string());
    while let Some(h) = queue.pop_front() {
        if !visited.insert(h.clone()) {
            continue;
        }
        if ancestors.contains(&h) {
            return Ok(Some(repo.snapshot(&h)?));
        }
        let snap = repo.snapshot(&h)?;
        queue.extend(snap.parent.iter().cloned());
    }
    Ok(None)
}

fn same_content(base: Option<&TreeEntry>, entry: &TreeEntry) -> bool {
    base.map(|b| b.hash == entry.hash).unwrap_or(false)
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

    // текстовое слияние

    #[test]
    fn текст_правки_в_разных_строках_объединяются() {
        let base = b"a\nb\nc\n";
        let ours = b"a\nB\nc\n";
        let theirs = b"a\nb\nC\n";
        let merged = merge_text_three_way(Some(base), ours, theirs)
            .expect("ошибки ввода-вывода нет")
            .expect("конфликта нет");
        assert_eq!(String::from_utf8(merged).expect("utf8"), "a\nB\nC\n");
    }

    #[test]
    fn текст_добавление_строк_с_обеих_сторон() {
        let base = b"head\n";
        let ours = "head\nнаша строка\n".as_bytes();
        let theirs = "head\nих строка\n".as_bytes();
        // обе дописали в конец по-разному — это конфликт
        let outcome = merge_text_three_way(Some(base), ours, theirs).expect("io");
        assert!(outcome.is_err(), "разные дописывания в одну точку — конфликт");
    }

    #[test]
    fn текст_одинаковые_правки_обеих_веток() {
        let base = b"x\n";
        let same = b"x\ny\n";
        let merged = merge_text_three_way(Some(base), same, same)
            .expect("io")
            .expect("конфликта нет");
        assert_eq!(merged, same.to_vec());
    }

    #[test]
    fn текст_правка_только_в_одной_ветке() {
        let base = b"1\n2\n3\n";
        let ours = b"1\n2\n3\n4\n";
        let theirs = b"1\n2\n3\n";
        let merged = merge_text_three_way(Some(base), ours, theirs)
            .expect("io")
            .expect("конфликта нет");
        assert_eq!(merged, ours.to_vec());
    }

    #[test]
    fn текст_конфликт_в_одной_строке() {
        let base = b"a\nb\nc\n";
        let ours = b"a\nX\nc\n";
        let theirs = b"a\nY\nc\n";
        let outcome = merge_text_three_way(Some(base), ours, theirs).expect("io");
        assert!(outcome.is_err());
    }

    #[test]
    fn сегменты_конфликта_несут_обе_стороны() {
        let base = b"a\nb\nc\n";
        let ours = b"a\nX\nc\n";
        let theirs = b"a\nY\nc\n";
        let segments = merge_text_segments(Some(base), ours, theirs);
        let conflicts: Vec<_> = segments
            .iter()
            .filter(|s| matches!(s, TextSegment::Conflict { .. }))
            .collect();
        assert_eq!(conflicts.len(), 1);
        if let TextSegment::Conflict { ours, theirs } = conflicts[0] {
            assert_eq!(ours.concat(), "X\n");
            assert_eq!(theirs.concat(), "Y\n");
        } else {
            unreachable!()
        }
    }

    // слияние CSV

    const CSV_BASE: &[u8] = b"gene_id,control,treatment,p_value\nBRCA1,10.2,15.7,0.0031\nTP53,8.4,8.1,0.77\n";
    const CSV_OURS: &[u8] = b"gene_id,control,treatment,p_value\nBRCA1,10.2,15.7,0.0031\nTP53,8.4,8.1,0.77\nMYC,3.2,9.9,0.012\n";
    const CSV_THEIRS: &[u8] = b"gene_id,control,treatment,p_value\nBRCA1,10.2,15.7,0.0031\nTP53,8.4,8.1,0.77\nEGFR,5.5,12.3,0.0007\n";

    #[test]
    fn csv_разные_строки_обеих_веток_объединяются() {
        let merged = merge_csv_three_way(Some(CSV_BASE), CSV_OURS, CSV_THEIRS)
            .expect("io")
            .expect("конфликта нет");
        let text = String::from_utf8(merged).expect("utf8");
        assert!(text.contains("MYC,3.2,9.9"), "наша добавленная строка: {text}");
        assert!(text.contains("EGFR,5.5,12.3"), "их добавленная строка: {text}");
        assert!(text.contains("BRCA1,10.2,15.7"), "общая строка сохранилась: {text}");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn csv_правки_разных_ячеек_одной_строки() {
        let base = "gene_id,value,note\nG1,1.0,исходное\n".as_bytes();
        let ours = "gene_id,value,note\nG1,2.0,исходное\n".as_bytes();
        let theirs = "gene_id,value,note\nG1,1.0,обновлено\n".as_bytes();
        let merged = merge_csv_three_way(Some(base), ours, theirs)
            .expect("io")
            .expect("конфликта нет");
        let text = String::from_utf8(merged).expect("utf8");
        assert!(text.contains("G1,2.0,обновлено"), "объединение ячеек: {text}");
    }

    #[test]
    fn csv_конфликт_одной_строки() {
        let base = b"gene_id,p_value\nG1,0.5\n";
        let ours = b"gene_id,p_value\nG1,0.1\n";
        let theirs = b"gene_id,p_value\nG1,0.9\n";
        let outcome = merge_csv_three_way(Some(base), ours, theirs).expect("io");
        let reason = outcome.expect_err("должен быть конфликт");
        assert!(reason.contains("G1"));
    }

    #[test]
    fn csv_удаление_строки_в_одной_ветке() {
        let base = b"gene_id,v\nG1,1\nG2,2\n";
        let ours = b"gene_id,v\nG1,1\nG2,2\n";
        let theirs = b"gene_id,v\nG1,1\n"; // удалили G2
        let merged = merge_csv_three_way(Some(base), ours, theirs)
            .expect("io")
            .expect("конфликта нет");
        let text = String::from_utf8(merged).expect("utf8");
        assert!(!text.contains("G2"), "строка G2 удалена: {text}");
    }

    #[test]
    fn csv_без_базы_одинаковые_добавления() {
        let same = b"gene_id,v\nG1,1\n";
        let merged = merge_csv_three_way(None, same, same).expect("io").expect("ок");
        assert_eq!(
            String::from_utf8(merged).expect("utf8"),
            String::from_utf8(same.to_vec()).expect("utf8")
        );
    }

    #[test]
    fn csv_настраиваемый_ключ_не_первая_колонка() {
        // ключ — колонка sample (вторая); правки в разных строках по этому ключу
        let base = "id,sample,value\n1,S1,10\n2,S2,20\n".as_bytes();
        let ours = "id,sample,value\n1,S1,11\n2,S2,20\n".as_bytes();
        let theirs = "id,sample,value\n1,S1,10\n2,S2,22\n".as_bytes();

        // без указания ключа (первая колонка «id» уникальна тоже) — сработает
        let by_default = merge_csv_with_details(Some(base), ours, theirs, None).expect("ок");
        assert!(by_default.conflicts.is_empty());

        // ключ «sample»: правки разных строк объединяются
        let by_sample = merge_csv_with_details(Some(base), ours, theirs, Some("sample")).expect("ок");
        assert!(by_sample.conflicts.is_empty());
        assert!(by_sample.rows.iter().any(|r| r == &["1", "S1", "11"]));
        assert!(by_sample.rows.iter().any(|r| r == &["2", "S2", "22"]));

        // несуществующая колонка — ошибка
        let err = merge_csv_with_details(Some(base), ours, theirs, Some("нет-колонки"));
        assert!(err.is_err());
    }

    #[test]
    fn csv_дубликат_ключа_отклоняется() {
        let base = "gene_id,v\nG1,1\n".as_bytes();
        let ours = "gene_id,v\nG1,1\nG1,2\n".as_bytes();
        let theirs = "gene_id,v\nG1,1\n".as_bytes();
        let outcome = merge_csv_three_way(Some(base), ours, theirs).expect("io");
        assert!(outcome.is_err(), "дубликат ключа должен давать ошибку");
    }

    // слияние ветвей целиком (старое поведение)

    #[test]
    fn слияние_двух_гипотез_без_конфликтов() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("expression.csv"), CSV_BASE).expect("файл");
        let s1 = repo.save_snapshot("база", "Дмитрий", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-B").expect("ветка");

        std::fs::write(repo.root().join("expression.csv"), CSV_OURS).expect("файл");
        let s2 = repo.save_snapshot("main: +MYC", "Дмитрий", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("hypothesis-B").expect("переключение");
        std::fs::write(repo.root().join("expression.csv"), CSV_THEIRS).expect("файл");
        let s3 = repo.save_snapshot("B: +EGFR", "Анна", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");
        let report = merge_branch(&repo, "hypothesis-B").expect("слияние");
        assert!(report.conflicts.is_empty(), "конфликты: {:?}", report.conflicts);
        let merged_snap = report.snapshot.as_ref().expect("снимок слияния");
        assert_eq!(merged_snap.parent, vec![s2.hash, s3.hash]);
        assert!(report.merged.iter().any(|f| f.starts_with("expression.csv")));

        let final_csv = std::fs::read_to_string(repo.root().join("expression.csv")).expect("итог");
        assert!(final_csv.contains("MYC,3.2,9.9"));
        assert!(final_csv.contains("EGFR,5.5,12.3"));

        let hist = repo.history("main").expect("история");
        assert_eq!(hist.len(), 3);
        assert_eq!(hist[0].hash, merged_snap.hash);
        let _ = s1;
    }

    #[test]
    fn слияние_с_конфликтом_отменяется_целиком() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("notes.txt"), "a\nb\nc\n").expect("файл");
        let s1 = repo.save_snapshot("база", "Дмитрий", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-X").expect("ветка");

        std::fs::write(repo.root().join("notes.txt"), "a\nX\nc\n").expect("файл");
        let _s2 = repo.save_snapshot("main: X", "Дмитрий", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("hypothesis-X").expect("переключение");
        std::fs::write(repo.root().join("notes.txt"), "a\nY\nc\n").expect("файл");
        let _s3 = repo.save_snapshot("X: Y", "Анна", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");
        let report = merge_branch(&repo, "hypothesis-X").expect("слияние");
        assert!(report.snapshot.is_none(), "конфликтное слияние не создаёт снимок");
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(report.conflicts[0].file, "notes.txt");

        assert_eq!(
            std::fs::read_to_string(repo.root().join("notes.txt")).expect("содержимое"),
            "a\nX\nc\n"
        );
        assert_eq!(repo.history("main").expect("история")[0].hash, _s2.hash);
        let _ = s1;
    }

    #[test]
    fn слияние_самой_с_собой_ничего_не_меняет() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("a.txt"), "1").expect("файл");
        let _ = repo.save_snapshot("снимок", "Аня", &serde_json::Value::Null).expect("снимок");
        let report = merge_branch(&repo, "main").expect("слияние");
        assert!(report.snapshot.is_some());
        assert!(report.merged.is_empty());
        assert_eq!(repo.history("main").expect("история").len(), 1);
    }

    #[test]
    fn двоичный_файл_изменён_с_обеих_сторон_конфликт() {
        let (_dir, repo) = repo_in_tmp();
        let base_bytes = [0xFFu8, 0xFE, 0x80, 0x00];
        std::fs::write(repo.root().join("sample.2bit"), base_bytes).expect("файл");
        let _s1 = repo.save_snapshot("база", "Дмитрий", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-Z").expect("ветка");

        std::fs::write(repo.root().join("sample.2bit"), [0xFFu8, 0xFE, 0x80, 0x01]).expect("файл");
        let _s2 = repo.save_snapshot("main: бинарник v2", "Дмитрий", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("hypothesis-Z").expect("переключение");
        std::fs::write(repo.root().join("sample.2bit"), [0xFFu8, 0xFE, 0x80, 0x02]).expect("файл");
        let _s3 = repo.save_snapshot("Z: бинарник v3", "Анна", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");
        let report = merge_branch(&repo, "hypothesis-Z").expect("слияние");
        assert_eq!(report.conflicts.len(), 1);
        assert!(report.conflicts[0].reason.contains("двоичный"));
    }

    #[test]
    fn слияние_несуществующей_ветки_даёт_ошибку() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("a.txt"), "1").expect("файл");
        let _ = repo.save_snapshot("снимок", "Аня", &serde_json::Value::Null).expect("снимок");
        let err = merge_branch(&repo, "нет-такой").expect_err("должна быть ошибка");
        assert!(matches!(err, BioGitError::BranchNotFound(_)));
    }

    #[test]
    fn защита_csv_экранированием() {
        assert_eq!(csv_escape("простое"), "простое");
        assert_eq!(csv_escape("с,запятой"), "\"с,запятой\"");
        assert_eq!(csv_escape("с\"кавычкой"), "\"с\"\"кавычкой\"");
    }

    // двухфазное слияние с разрешением конфликтов

    #[test]
    fn предложение_слияния_не_меняет_репозиторий() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("notes.txt"), "a\nb\nc\n").expect("файл");
        let _s1 = repo.save_snapshot("база", "Дмитрий", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-X").expect("ветка");

        std::fs::write(repo.root().join("notes.txt"), "a\nX\nc\n").expect("файл");
        let s2 = repo.save_snapshot("main: X", "Дмитрий", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("hypothesis-X").expect("переключение");
        std::fs::write(repo.root().join("notes.txt"), "a\nY\nc\n").expect("файл");
        let _s3 = repo.save_snapshot("X: Y", "Анна", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");
        let proposal = propose_merge(&repo, "hypothesis-X", &MergeOptions::default()).expect("предложение");
        assert!(proposal.has_conflicts());
        assert_eq!(proposal.conflict_files(), vec!["notes.txt"]);

        // репозиторий не тронут
        assert_eq!(
            std::fs::read_to_string(repo.root().join("notes.txt")).expect("содержимое"),
            "a\nX\nc\n"
        );
        assert_eq!(repo.history("main").expect("история")[0].hash, s2.hash);
    }

    #[test]
    fn конфликт_текста_разрешается_в_интерфейсе_наша_версия() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("notes.txt"), "a\nb\nc\n").expect("файл");
        let _s1 = repo.save_snapshot("база", "Дмитрий", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-X").expect("ветка");

        std::fs::write(repo.root().join("notes.txt"), "a\nX\nc\n").expect("файл");
        let _s2 = repo.save_snapshot("main: X", "Дмитрий", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("hypothesis-X").expect("переключение");
        std::fs::write(repo.root().join("notes.txt"), "a\nY\nc\n").expect("файл");
        let _s3 = repo.save_snapshot("X: Y", "Анна", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");
        let proposal = propose_merge(&repo, "hypothesis-X", &MergeOptions::default()).expect("предложение");
        let file = &proposal.files[0];
        if let ConflictDetail::TextHunk { index, ours, theirs } = &file.conflicts[0] {
            assert_eq!(*index, 0);
            assert_eq!(ours.concat(), "X\n");
            assert_eq!(theirs.concat(), "Y\n");
        } else {
            panic!("ожидался текстовый конфликт");
        }

        // решение: взять «их» строку
        let decisions = vec![FileDecision {
            path: "notes.txt".to_string(),
            action: "merged".to_string(),
            hunks: [(0usize, SIDE_THEIRS.to_string())].into_iter().collect(),
            ..Default::default()
        }];
        let report = apply_merge(&repo, "hypothesis-X", &MergeOptions::default(), &decisions).expect("apply");
        assert!(report.snapshot.is_some());
        assert_eq!(
            std::fs::read_to_string(repo.root().join("notes.txt")).expect("итог"),
            "a\nY\nc\n"
        );
    }

    #[test]
    fn конфликт_csv_ячеек_разрешается_поячеечно() {
        let (_dir, repo) = repo_in_tmp();
        let base = b"gene_id,control,treatment\nTP53,8.4,8.1\nBRCA1,10.2,15.7\n";
        std::fs::write(repo.root().join("expr.csv"), base).expect("файл");
        let _s1 = repo.save_snapshot("база", "Дмитрий", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-B").expect("ветка");

        // main правит control, ветка правит treatment
        std::fs::write(repo.root().join("expr.csv"), b"gene_id,control,treatment\nTP53,9.9,8.1\nBRCA1,10.2,15.7\n").expect("файл");
        let _s2 = repo.save_snapshot("main: control", "Дмитрий", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("hypothesis-B").expect("переключение");
        std::fs::write(repo.root().join("expr.csv"), b"gene_id,control,treatment\nTP53,8.4,12.0\nBRCA1,10.2,15.7\n").expect("файл");
        let _s3 = repo.save_snapshot("B: treatment", "Анна", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");

        // разные колонки — конфликтов нет, всё объединится
        let proposal = propose_merge(&repo, "hypothesis-B", &MergeOptions::default()).expect("предложение");
        assert!(!proposal.has_conflicts(), "правки разных колонок сливаются: {:?}", proposal.files[0].conflicts);
        let _report = apply_merge(&repo, "hypothesis-B", &MergeOptions::default(), &[]).expect("apply");
        assert_eq!(
            std::fs::read_to_string(repo.root().join("expr.csv")).expect("итог"),
            "gene_id,control,treatment\nBRCA1,10.2,15.7\nTP53,9.9,12.0\n",
            "обе правки в итоговой строке"
        );

        // теперь создаём настоящий конфликт ячейки
        repo.create_branch("hypothesis-C").expect("ветка C");
        repo.switch_branch("hypothesis-C").expect("переключение");
        std::fs::write(repo.root().join("expr.csv"), b"gene_id,control,treatment\nTP53,1.1,12.0\nBRCA1,10.2,15.7\n").expect("файл");
        let _sc = repo.save_snapshot("C: control 1.1", "Анна", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");
        std::fs::write(repo.root().join("expr.csv"), b"gene_id,control,treatment\nTP53,2.2,12.0\nBRCA1,10.2,15.7\n").expect("файл");
        let _sm = repo.save_snapshot("main: control 2.2", "Дмитрий", &serde_json::Value::Null).expect("снимок");

        let proposal = propose_merge(&repo, "hypothesis-C", &MergeOptions::default()).expect("предложение");
        assert!(proposal.has_conflicts());
        let cell = match &proposal.files[0].conflicts[0] {
            ConflictDetail::CsvCell { row_key, column, ours, theirs } => {
                assert_eq!(row_key, "TP53");
                assert_eq!(column, "control");
                assert_eq!(ours, "2.2");
                assert_eq!(theirs, "1.1");
                format!("{row_key}\u{1f}{column}")
            }
            other => panic!("ожидался конфликт ячейки, получено: {other:?}"),
        };

        // решение: взять «их» значение
        let decisions = vec![FileDecision {
            path: "expr.csv".to_string(),
            action: "merged".to_string(),
            cells: [(cell, SIDE_THEIRS.to_string())].into_iter().collect(),
            ..Default::default()
        }];
        let report = apply_merge(&repo, "hypothesis-C", &MergeOptions::default(), &decisions).expect("apply");
        let final_csv = std::fs::read_to_string(repo.root().join("expr.csv")).expect("итог");
        assert!(final_csv.contains("TP53,1.1,12.0"), "выбрано их значение: {final_csv}");
        assert_eq!(report.snapshot.expect("снимок").parent.len(), 2);
    }

    #[test]
    fn конфликт_строки_csv_удаление_против_правки() {
        let (_dir, repo) = repo_in_tmp();
        let base = b"gene_id,v\nG1,1\nG2,2\n";
        std::fs::write(repo.root().join("data.csv"), base).expect("файл");
        let _s1 = repo.save_snapshot("база", "Дмитрий", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-D").expect("ветка");

        // main правит G2
        std::fs::write(repo.root().join("data.csv"), b"gene_id,v\nG1,1\nG2,3\n").expect("файл");
        let _s2 = repo.save_snapshot("main: G2=3", "Дмитрий", &serde_json::Value::Null).expect("снимок");

        // ветка удаляет G2
        repo.switch_branch("hypothesis-D").expect("переключение");
        std::fs::write(repo.root().join("data.csv"), b"gene_id,v\nG1,1\n").expect("файл");
        let _s3 = repo.save_snapshot("D: удалили G2", "Анна", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");
        let proposal = propose_merge(&repo, "hypothesis-D", &MergeOptions::default()).expect("предложение");
        assert!(proposal.has_conflicts());

        // решение: строка остаётся (наша версия)
        let decisions = vec![FileDecision {
            path: "data.csv".to_string(),
            action: "merged".to_string(),
            rows: [("G2".to_string(), SIDE_OURS.to_string())].into_iter().collect(),
            ..Default::default()
        }];
        apply_merge(&repo, "hypothesis-D", &MergeOptions::default(), &decisions).expect("apply");
        let final_csv = std::fs::read_to_string(repo.root().join("data.csv")).expect("итог");
        assert!(final_csv.contains("G2,3"), "строка сохранена с нашей правкой: {final_csv}");
    }

    #[test]
    fn конфликт_двоичного_файла_разрешается_целиком() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("sample.2bit"), [0xFFu8, 0xFE, 0x80, 0x00]).expect("файл");
        let _s1 = repo.save_snapshot("база", "Дмитрий", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-Z").expect("ветка");

        std::fs::write(repo.root().join("sample.2bit"), [0xFFu8, 0xFE, 0x80, 0x01]).expect("файл");
        let _s2 = repo.save_snapshot("main: v2", "Дмитрий", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("hypothesis-Z").expect("переключение");
        std::fs::write(repo.root().join("sample.2bit"), [0xFFu8, 0xFE, 0x80, 0x02]).expect("файл");
        let _s3 = repo.save_snapshot("Z: v3", "Анна", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");
        let proposal = propose_merge(&repo, "hypothesis-Z", &MergeOptions::default()).expect("предложение");
        assert!(proposal.has_conflicts());
        assert!(matches!(proposal.files[0].conflicts[0], ConflictDetail::FileLevel { .. }));

        let decisions = vec![FileDecision {
            path: "sample.2bit".to_string(),
            action: SIDE_THEIRS.to_string(),
            ..Default::default()
        }];
        apply_merge(&repo, "hypothesis-Z", &MergeOptions::default(), &decisions).expect("apply");
        let bytes = std::fs::read(repo.root().join("sample.2bit")).expect("итог");
        assert_eq!(bytes, vec![0xFFu8, 0xFE, 0x80, 0x02], "взята их версия");
    }

    #[test]
    fn слияние_с_настраиваемым_ключом_csv() {
        let (_dir, repo) = repo_in_tmp();
        let base = "id,sample,value\n1,S1,10\n2,S2,20\n".as_bytes();
        std::fs::write(repo.root().join("table.csv"), base).expect("файл");
        let _s1 = repo.save_snapshot("база", "Дмитрий", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-K").expect("ветка");

        std::fs::write(repo.root().join("table.csv"), "id,sample,value\n1,S1,11\n2,S2,20\n".as_bytes()).expect("файл");
        let _s2 = repo.save_snapshot("main: S1=11", "Дмитрий", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("hypothesis-K").expect("переключение");
        std::fs::write(repo.root().join("table.csv"), "id,sample,value\n1,S1,10\n2,S2,22\n".as_bytes()).expect("файл");
        let _s3 = repo.save_snapshot("K: S2=22", "Анна", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");
        let options = MergeOptions { csv_key: Some("sample".to_string()) };
        let report = merge_branch_with_options(&repo, "hypothesis-K", &options).expect("слияние");
        assert!(report.snapshot.is_some(), "конфликтов не должно быть");
        let final_csv = std::fs::read_to_string(repo.root().join("table.csv")).expect("итог");
        assert!(final_csv.contains("S1,11"), "правка main: {final_csv}");
        assert!(final_csv.contains("S2,22"), "правка ветки: {final_csv}");
    }

    #[test]
    fn удаление_файла_в_источнике_применяется() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("a.txt"), "1").expect("файл");
        std::fs::write(repo.root().join("b.txt"), "2").expect("файл");
        let _s1 = repo.save_snapshot("база", "Дмитрий", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-R").expect("ветка");

        std::fs::remove_file(repo.root().join("b.txt")).expect("удаление");
        let _s2 = repo.save_snapshot("main: без b", "Дмитрий", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("hypothesis-R").expect("переключение");
        std::fs::write(repo.root().join("a.txt"), "1-ветка").expect("файл");
        let _s3 = repo.save_snapshot("R: правка a", "Анна", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");
        let report = merge_branch(&repo, "hypothesis-R").expect("слияние");
        assert!(report.snapshot.is_some());
        assert!(!repo.root().join("b.txt").exists(), "b.txt удалён слиянием");
        assert_eq!(
            std::fs::read_to_string(repo.root().join("a.txt")).expect("итог"),
            "1-ветка",
            "правка ветки принесена"
        );
    }
}

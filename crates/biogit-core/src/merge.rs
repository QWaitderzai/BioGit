//! Умное слияние ветвей-гипотез.
//!
//! Три уровня разрешений конфликтов:
//! 1. Снимок целиком — сравнение по хешам содержимого.
//! 2. Текстовые файлы — трёхстороннее построчное слияние.
//! 3. CSV — трёхстороннее слияние по ключевой колонке (например `gene_id`)
//!    с разбором через Polars.
//!
//! Если хотя бы одно расхождение неразрешимо, слияние отменяется целиком
//! (репозиторий остаётся в исходном состоянии), а отчёт перечисляет конфликты.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::io::Write as _;

use serde::Serialize;

use crate::error::{BioGitError, Result};
use crate::object::{Snapshot, Tree, TreeEntry};
use crate::snapshot::Repo;

/// Отдельное неразрешимое расхождение.
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

/// Выполняет слияние ветки `source` в текущую ветку репозитория.
pub fn merge_branch(repo: &Repo, source: &str) -> Result<MergeReport> {
    crate::snapshot::validate_branch_name(source)?;
    let target_branch = repo.head()?;
    let ours = repo.current_commit()?.ok_or_else(|| {
        BioGitError::InvalidState(format!("на ветке «{target_branch}» ещё нет снимков"))
    })?;
    let theirs = repo.snapshot_of_branch(source)?;
    if theirs.hash == ours.hash {
        return Ok(MergeReport {
            source: source.to_string(),
            target: target_branch,
            merged: Vec::new(),
            conflicts: Vec::new(),
            snapshot: Some(ours),
        });
    }

    // 1. Общий предок — база трёхстороннего слияния
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

    // 2. Разбор каждого файла
    let mut result_entries: Vec<TreeEntry> = Vec::new();
    let mut merged: Vec<String> = Vec::new();
    let mut conflicts: Vec<ConflictItem> = Vec::new();

    for path in paths {
        let b = base_map.get(path).copied();
        let o = ours_map.get(path).copied();
        let t = theirs_map.get(path).copied();

        match (o, t) {
            (Some(oe), Some(te)) if oe.hash == te.hash => result_entries.push(oe.clone()),
            (Some(oe), Some(te)) if same_content(b, te) => {
                result_entries.push(oe.clone()); // меняли только у нас
            }
            (Some(oe), Some(te)) if same_content(b, oe) => {
                result_entries.push(te.clone()); // меняли только в источнике
                merged.push(path.to_string());
            }
            (Some(oe), Some(te)) => {
                // изменён с обеих сторон — пробуем умное слияние содержимого
                resolve_content(
                    repo,
                    path,
                    b,
                    oe,
                    te,
                    &mut result_entries,
                    &mut merged,
                    &mut conflicts,
                )?;
            }
            (Some(oe), None) => {
                if b.is_none() {
                    result_entries.push(oe.clone()); // добавлен только у нас
                } else if same_content(b, oe) {
                    merged.push(format!("{path} (удалён в источнике)"));
                } else {
                    conflicts.push(ConflictItem {
                        file: path.to_string(),
                        reason: "файл изменён в целевой ветке и удалён в источнике".to_string(),
                    });
                }
            }
            (None, Some(te)) => {
                if b.is_none() {
                    result_entries.push(te.clone()); // добавлен только в источнике
                    merged.push(path.to_string());
                } else if same_content(b, te) {
                    merged.push(format!("{path} (удалён у нас)"));
                } else {
                    conflicts.push(ConflictItem {
                        file: path.to_string(),
                        reason: "файл удалён в целевой ветке и изменён в источнике".to_string(),
                    });
                }
            }
            (None, None) => {} // удалён в обеих ветках
        }
    }

    // 3. При конфликтах слияние отменяется целиком
    if !conflicts.is_empty() {
        return Ok(MergeReport {
            source: source.to_string(),
            target: target_branch,
            merged,
            conflicts,
            snapshot: None,
        });
    }

    // 4. Снимок слияния с двумя родителями
    result_entries.sort_by(|a, b| a.name.cmp(&b.name));
    let tree = Tree {
        entries: result_entries,
    };
    let tree_hash = repo.store_tree(&tree)?;
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let message = format!("Слияние ветви гипотезы «{source}»");
    let parent = vec![ours.hash.clone(), theirs.hash.clone()];
    let files_count = tree.entries.len();
    let hash = Snapshot::compute_hash(
        &timestamp,
        &ours.author,
        &message,
        &parent,
        &target_branch,
        &tree_hash,
        &ours.parameters,
        files_count,
    );
    let snapshot = Snapshot {
        hash: hash.clone(),
        timestamp,
        author: ours.author.clone(),
        message,
        parent,
        branch: target_branch.clone(),
        tree: tree_hash,
        parameters: ours.parameters.clone(),
        files_count,
    };
    repo.write_commit_object(&snapshot)?;
    repo.set_branch_ref(&target_branch, &hash)?;

    // 5. Обновляем рабочую папку до состояния слияния
    repo.restore(&hash)?;

    Ok(MergeReport {
        source: source.to_string(),
        target: target_branch,
        merged,
        conflicts: Vec::new(),
        snapshot: Some(snapshot),
    })
}

fn same_content(base: Option<&TreeEntry>, entry: &TreeEntry) -> bool {
    base.map(|b| b.hash == entry.hash).unwrap_or(false)
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

/// Пытается разрешить содержимое файла, изменённого в обеих ветках.
#[allow(clippy::too_many_arguments)]
fn resolve_content(
    repo: &Repo,
    path: &str,
    base: Option<&TreeEntry>,
    ours: &TreeEntry,
    theirs: &TreeEntry,
    out_entries: &mut Vec<TreeEntry>,
    merged: &mut Vec<String>,
    conflicts: &mut Vec<ConflictItem>,
) -> Result<()> {
    let base_bytes = match base {
        Some(e) => Some(repo.read_blob(&e.hash)?),
        None => None,
    };
    let ours_bytes = repo.read_blob(&ours.hash)?;
    let theirs_bytes = repo.read_blob(&theirs.hash)?;

    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let outcome = if matches!(ext.as_str(), "csv" | "tsv") {
        merge_csv_three_way(base_bytes.as_deref(), &ours_bytes, &theirs_bytes)?
    } else {
        let base_text = base_bytes
            .as_deref()
            .and_then(|b| std::str::from_utf8(b).ok())
            .map(str::as_bytes);
        let ours_text = std::str::from_utf8(&ours_bytes).ok();
        let theirs_text = std::str::from_utf8(&theirs_bytes).ok();
        match (ours_text, theirs_text) {
            (Some(o), Some(t)) => merge_text_three_way(base_text, o.as_bytes(), t.as_bytes())?,
            _ => Err("двоичный файл изменён в обеих ветках".to_string()),
        }
    };

    match outcome {
        Ok(content) => {
            let hash = repo.store().put_bytes(&content)?;
            out_entries.push(TreeEntry {
                name: path.to_string(),
                hash,
                size: content.len() as u64,
            });
            merged.push(path.to_string());
        }
        Err(reason) => conflicts.push(ConflictItem {
            file: path.to_string(),
            reason,
        }),
    }
    Ok(())
}

// 
// Текстовое слияние (хунковый трёхсторонний алгоритм, как в git)


/// Итог трёхстороннего слияния содержимого.
pub(crate) type ContentMergeOutcome = std::result::Result<Vec<u8>, String>;

/// Хунок изменения: base[z0..z1) заменено на other[s0..s1).
type Hunk = (usize, usize, usize, usize);

/// Трёхстороннее построчное слияние текста.
///
/// Строим два набора изменённых областей: base→наши и base→их.
/// Непересекающиеся области применяются независимо (правки соседних строк
/// объединяются), пересекающиеся — сравниваются по результату: одинаковые
/// применяются один раз, разные дают конфликт.
pub(crate) fn merge_text_three_way(
    base: Option<&[u8]>,
    ours: &[u8],
    theirs: &[u8],
) -> Result<ContentMergeOutcome> {
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

    let mut out: Vec<String> = Vec::new();
    let mut conflicts = 0usize;
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
            out.extend_from_slice(&b[p..start]);
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
        // Строго пересекающиеся хунки присоединяем, пока группа растёт
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
            out.extend(their_text); // в области меняли только «их»
        } else if t_from == it {
            out.extend(our_text); // в области меняли только «наши»
        } else if our_text == their_text {
            out.extend(our_text); // обе ветки изменили одинаково
        } else {
            conflicts += 1; // черновой вывод не используется: слияние отменяется
        }
        p = end;
    }
    out.extend_from_slice(&b[p..]);

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
fn lcs_matches(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
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


// Слияние CSV по ключевой колонке


/// Таблица CSV: заголовок + строки (значения — текст).
type CsvTable = (Vec<String>, Vec<Vec<String>>);

/// Трёхстороннее слияние CSV по ключевой колонке (первая колонка, `gene_id`).
/// Разбор выполняет Polars — корректно обрабатываются кавычки и типы.
pub(crate) fn merge_csv_three_way(
    base: Option<&[u8]>,
    ours: &[u8],
    theirs: &[u8],
) -> Result<ContentMergeOutcome> {
    let ours_table = read_csv_table(ours, "рабочая копия")?;
    let theirs_table = read_csv_table(theirs, "ветвь-источник")?;
    let base_table = match base {
        Some(bytes) => Some(read_csv_table(bytes, "базовый снимок")?),
        None => None,
    };

    match merge_tables(base_table.as_ref(), &ours_table, &theirs_table) {
        Err(reason) => Ok(Err(reason)),
        Ok(rows) => {
            let mut out = Vec::new();
            push_row(&ours_table.0, &mut out);
            for row in &rows {
                push_row(row, &mut out);
            }
            Ok(Ok(out))
        }
    }
}

fn read_csv_table(bytes: &[u8], label: &str) -> Result<CsvTable> {
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
            // Строки извлекаем через str(): Display AnyValue оборачивает их в кавычки
            let text = if series.dtype().is_string() {
                match series.str().map_err(BioGitError::from)?.get(i) {
                    Some(value) => value.to_string(),
                    None => String::new(), // пустая ячейка Polars отдаёт как null
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

fn merge_tables(
    base: Option<&CsvTable>,
    ours: &CsvTable,
    theirs: &CsvTable,
) -> std::result::Result<Vec<Vec<String>>, String> {
    let (header, ours_rows) = ours;
    let (_, theirs_rows) = theirs;

    if header.is_empty() {
        return Err("CSV без колонок".to_string());
    }
    if ours.0 != theirs.0 {
        return Err("структура колонок изменена в обеих ветках по-разному".to_string());
    }

    let key_idx = 0usize; // ключевая колонка — первая (gene_id)
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

    let mut out = Vec::new();
    for key in keys {
        let b = bm.get(&key);
        let o = om.get(&key);
        let t = tm.get(&key);
        let resolved = match (o, t) {
            (Some(x), Some(y)) if x == y => Some(x.clone()),
            (Some(x), Some(y)) => {
                if b == Some(y) {
                    Some(x.clone()) // меняли только у нас
                } else if b == Some(x) {
                    Some(y.clone()) // меняли только в источнике
                } else {
                    // обе ветки изменили строку — пробуем поячеечное слияние
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
                                    let col = header.get(c).map(String::as_str).unwrap_or("?");
                                    return Err(format!(
                                        "строка «{key}», колонка «{col}»: изменена в обеих ветках по-разному"
                                    ));
                                }
                            }
                            Some(row)
                        }
                        _ => {
                            return Err(format!(
                                "строка «{key}» добавлена в обеих ветках с разным содержимым"
                            ));
                        }
                    }
                }
            }
            (Some(x), None) => {
                if b.is_none() {
                    Some(x.clone()) // добавлено только у нас
                } else if b == Some(x) {
                    None // удалено только в источнике
                } else {
                    return Err(format!("строка «{key}» изменена в целевой ветке и удалена в источнике"));
                }
            }
            (None, Some(y)) => {
                if b.is_none() {
                    Some(y.clone()) // добавлено только в источнике
                } else if b == Some(y) {
                    None // удалено у нас, в источнике не меняли
                } else {
                    return Err(format!("строка «{key}» удалена в целевой ветке и изменена в источнике"));
                }
            }
            (None, None) => None, // удалено в обеих ветках
        };
        if let Some(row) = resolved {
            out.push(row);
        }
    }
    Ok(out)
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
    fn текст_без_базы_добавлен_одинаково() {
        let added = "новый файл\nстрока 2\n".as_bytes();
        let merged = merge_text_three_way(None, added, added).expect("io").expect("ок");
        assert_eq!(merged, added.to_vec());
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
        // строки отсортированы по ключу: BRCA1, EGFR, TP53, MYC
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
        assert_eq!(String::from_utf8(merged).expect("utf8"), String::from_utf8(same.to_vec()).expect("utf8"));
    }

    // слияние ветвей целиком

    #[test]
    fn слияние_двух_гипотез_без_конфликтов() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("expression.csv"), CSV_BASE).expect("файл");
        let s1 = repo.save_snapshot("база", "Дмитрий", &serde_json::Value::Null).expect("снимок");
        repo.create_branch("hypothesis-B").expect("ветка");

        // main: добавили MYC
        std::fs::write(repo.root().join("expression.csv"), CSV_OURS).expect("файл");
        let s2 = repo.save_snapshot("main: +MYC", "Дмитрий", &serde_json::Value::Null).expect("снимок");

        // hypothesis-B: добавили EGFR
        repo.switch_branch("hypothesis-B").expect("переключение");
        std::fs::write(repo.root().join("expression.csv"), CSV_THEIRS).expect("файл");
        let s3 = repo.save_snapshot("B: +EGFR", "Анна", &serde_json::Value::Null).expect("снимок");

        repo.switch_branch("main").expect("переключение");
        let report = merge_branch(&repo, "hypothesis-B").expect("слияние");
        assert!(report.conflicts.is_empty(), "конфликты: {:?}", report.conflicts);
        let merged_snap = report.snapshot.as_ref().expect("снимок слияния");
        assert_eq!(merged_snap.parent, vec![s2.hash, s3.hash]);
        assert!(report.merged.iter().any(|f| f.starts_with("expression.csv")));

        // рабочая папка обновлена содержимым обеих веток
        let final_csv = std::fs::read_to_string(repo.root().join("expression.csv")).expect("итог");
        assert!(final_csv.contains("MYC,3.2,9.9"));
        assert!(final_csv.contains("EGFR,5.5,12.3"));

        // история main линейна и содержит 3 снимка
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

        // рабочая папка и ссылка main не изменились
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
        // байты, не являющиеся корректным UTF-8 (иначе файл пройдёт как «текст»)
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
}

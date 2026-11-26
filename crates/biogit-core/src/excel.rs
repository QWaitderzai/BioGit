//! Работа с электронными таблицами: Excel как вычислительный движок (уровень A).
//!
//! Схема «Excel как движок»: приложение запрашивает у системы объект
//! `Excel.Application` через COM-автоматизацию — Windows сама находит
//! установленный Excel через реестр (включая Microsoft 365, где файлы лежат
//! в виртуализованном `C:\Program Files\Microsoft Office\root\Office16`).
//! Excel запускается невидимым процессом, формулы считает он сам; программа
//! рисует собственную таблицу и общается с Excel через JSON-протокол
//! поверх PowerShell-моста (`excel_bridge.ps1`).
//!
//! Если Excel недоступен (не установлен / не Windows), автоматически
//! используется встроенный режим: чтение/запись xlsx через `calamine`
//! и `rust_xlsxwriter`, а формулы вычисляет примитивный [`crate::formula`].

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::error::{BioGitError, Result};
use crate::formula;
use crate::snapshot::Repo;

/// Лимит клеток в одной книге для встроенного режима.
const MAX_SHEET_CELLS: usize = 500_000;

/// Допустимые расширения табличных файлов.
const SHEET_EXTENSIONS: [&str; 5] = ["xlsx", "xls", "xlsm", "xlsb", "ods"];

/// Таймауты операций моста.
const TIMEOUT_PING: Duration = Duration::from_secs(30);
const TIMEOUT_OPEN: Duration = Duration::from_secs(45);
const TIMEOUT_READ: Duration = Duration::from_secs(30);
const TIMEOUT_EDIT: Duration = Duration::from_secs(15);
const TIMEOUT_SAVE: Duration = Duration::from_secs(45);
const TIMEOUT_CLOSE: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Модель данных
// ---------------------------------------------------------------------------

/// Ячейка таблицы: отображаемое значение + формула (с ведущим `=`).
#[derive(Debug, Clone, Serialize)]
pub struct SheetCell {
    pub value: String,
    pub formula: Option<String>,
}

/// Один лист книги.
#[derive(Debug, Clone, Serialize)]
pub struct SheetGrid {
    pub name: String,
    pub rows: Vec<Vec<SheetCell>>,
}

/// Документ: книга целиком + состояние сессии.
#[derive(Debug, Clone, Serialize)]
pub struct SheetDoc {
    /// Имя файла в рабочей папке (может измениться при сохранении из builtin).
    pub file: String,
    pub sheets: Vec<SheetGrid>,
    pub active_sheet: usize,
    /// "excel" — считает настоящий Excel; "builtin" — примитивный встроенный режим.
    pub engine: String,
    /// Пояснение режима для интерфейса.
    pub note: Option<String>,
    pub dirty: bool,
}

/// Редактирование ячейки: результат для интерфейса.
#[derive(Debug, Clone, Serialize)]
pub struct EditOutcome {
    pub value: String,
    pub formula: Option<String>,
}

/// Информация о доступности Excel на машине.
#[derive(Debug, Clone, Serialize)]
pub struct ExcelAvailability {
    pub available: bool,
    pub reason: Option<String>,
    pub version: Option<String>,
}

/// Сессия работы с одной книгой. Держится в состоянии Tauri.
pub struct SheetSession {
    pub doc: SheetDoc,
    backend: Backend,
    repo_root: PathBuf,
}

enum Backend {
    Excel(ExcelBridge),
    Builtin,
}

// ---------------------------------------------------------------------------
// Открытие / закрытие
// ---------------------------------------------------------------------------

fn validate_name(name: &str) -> Result<()> {
    let dangerous = name.is_empty()
        || name.starts_with('/')
        || name.contains('\\')
        || name.contains(':')
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    if dangerous {
        return Err(BioGitError::UnsafePath(name.to_string()));
    }
    Ok(())
}

fn extension_of(name: &str) -> String {
    name.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

/// Открывает книгу: сначала пытается подключить настоящий Excel (COM),
/// при неудаче автоматически переходит во встроенный режим.
pub fn sheet_open(repo: &Repo, name: &str) -> Result<SheetSession> {
    validate_name(name)?;
    let ext = extension_of(name);
    if !SHEET_EXTENSIONS.contains(&ext.as_str()) {
        return Err(BioGitError::InvalidState(format!(
            "«{name}» — не табличный файл (xlsx/xls/xlsm/xlsb/ods)"
        )));
    }
    let full_path = repo.root().join(name);
    if !full_path.exists() {
        return Err(BioGitError::InvalidState(format!("файл не найден: {name}")));
    }

    // Путь 1: настоящий Excel как вычислительный движок
    if let Ok(mut bridge) = spawn_bridge() {
        match open_with_excel(&mut bridge, &full_path, name) {
            Ok(doc) => {
                return Ok(SheetSession {
                    doc,
                    backend: Backend::Excel(bridge),
                    repo_root: repo.root().to_path_buf(),
                });
            }
            Err(_) => {
                // мост не взлетел — тихо переходим во встроенный режим
                drop(bridge);
            }
        }
    }

    // Путь 2: встроенный режим без Excel
    let doc = open_builtin(&full_path, name)?;
    Ok(SheetSession {
        doc,
        backend: Backend::Builtin,
        repo_root: repo.root().to_path_buf(),
    })
}

/// Смена активного листа: в Excel-режиме лист перечитывается из книги.
pub fn sheet_set_active(session: &mut SheetSession, index: usize) -> Result<()> {
    if index >= session.doc.sheets.len() {
        return Err(BioGitError::InvalidState("лист не найден".into()));
    }
    let file = session.doc.file.clone();
    let repo_root = session.repo_root.clone();
    let sheet_name = session.doc.sheets[index].name.clone();
    match &mut session.backend {
        Backend::Excel(bridge) => {
            let full_path = repo_root.join(&file);
            let grid = read_sheet_via_excel(bridge, &full_path, &sheet_name)?;
            session.doc.sheets[index] = grid;
        }
        Backend::Builtin => {}
    }
    session.doc.active_sheet = index;
    Ok(())
}

/// Редактирование ячейки: `input` — новое содержимое ("" — очистка,
/// текст с "=" — формула, остальное — литеральное значение).
pub fn sheet_edit(
    session: &mut SheetSession,
    sheet_index: usize,
    row: usize,
    col: usize,
    input: &str,
) -> Result<EditOutcome> {
    if sheet_index >= session.doc.sheets.len() {
        return Err(BioGitError::InvalidState("лист не найден".into()));
    }
    let file = session.doc.file.clone();
    let repo_root = session.repo_root.clone();
    let sheet_name = session.doc.sheets[sheet_index].name.clone();

    match &mut session.backend {
        Backend::Excel(bridge) => {
            let full_path = repo_root.join(&file);
            let (value, formula) = bridge.edit(&full_path, &sheet_name, row, col, input)?;
            let grid = &mut session.doc.sheets[sheet_index];
            ensure_capacity(grid, row + 1, col + 1);
            let cell = &mut grid.rows[row][col];
            cell.value = value.clone();
            cell.formula = formula.clone();
            session.doc.dirty = true;
            Ok(EditOutcome { value, formula })
        }
        Backend::Builtin => {
            let grid = &mut session.doc.sheets[sheet_index];
            ensure_capacity(grid, row + 1, col + 1);
            let cell = &mut grid.rows[row][col];
            if input.is_empty() {
                cell.value.clear();
                cell.formula = None;
            } else if input.starts_with('=') {
                let normalized = format!("={}", input.trim_start_matches('='));
                cell.formula = Some(normalized);
                cell.value = String::new();
            } else {
                cell.value = input.to_string();
                cell.formula = None;
            }
            // пересчёт всех формул листа примитивным движком
            formula::evaluate_sheet(grid);
            let value = grid.rows[row][col].value.clone();
            let formula = grid.rows[row][col].formula.clone();
            session.doc.dirty = true;
            Ok(EditOutcome { value, formula })
        }
    }
}

/// Сохраняет книгу. Встроенный режим для .xls/.ods и т.п. сохраняет в .xlsx
/// (имя файла обновляется). Возвращает актуальное имя файла.
pub fn sheet_save(session: &mut SheetSession, repo: &Repo) -> Result<String> {
    match &mut session.backend {
        Backend::Excel(bridge) => {
            let full_path = repo.root().join(&session.doc.file);
            bridge.save(&full_path)?;
            session.doc.dirty = false;
            Ok(session.doc.file.clone())
        }
        Backend::Builtin => {
            let ext = extension_of(&session.doc.file);
            if ext != "xlsx" {
                let stem = session
                    .doc
                    .file
                    .rsplit_once('.')
                    .map(|(stem, _)| stem.to_string())
                    .unwrap_or_else(|| session.doc.file.clone());
                let new_name = format!("{stem}.xlsx");
                session.doc.file = new_name;
                session.doc.note = Some(
                    "Встроенный режим сохраняет только в .xlsx — файл записан в новом формате."
                        .to_string(),
                );
            }
            save_builtin(session, repo)?;
            session.doc.dirty = false;
            Ok(session.doc.file.clone())
        }
    }
}

/// Закрывает сессию: в Excel-режиме книга закрывается без сохранения
/// и процесс Excel завершается.
pub fn sheet_close(session: &mut SheetSession) {
    if let Backend::Excel(bridge) = &mut session.backend {
        let file = session.doc.file.clone();
        let repo_root = session.repo_root.clone();
        let full_path = repo_root.join(&file);
        let _ = bridge.close_workbook(&full_path);
        let _ = bridge.request("quit", serde_json::json!({}), TIMEOUT_CLOSE);
    }
}

/// Есть ли несохранённые изменения.
pub fn sheet_dirty(session: &SheetSession) -> bool {
    session.doc.dirty
}

// ---------------------------------------------------------------------------
// Excel-режим: работа через мост
// ---------------------------------------------------------------------------

fn open_with_excel(bridge: &mut ExcelBridge, full_path: &Path, name: &str) -> Result<SheetDoc> {
    bridge.ping()?;
    let sheets = bridge.open(&full_path.to_string_lossy())?;
    if sheets.is_empty() {
        return Err(BioGitError::Excel("в книге нет листов".into()));
    }
    let mut doc_sheets: Vec<SheetGrid> = sheets
        .iter()
        .map(|sheet_name| SheetGrid {
            name: sheet_name.clone(),
            rows: Vec::new(),
        })
        .collect();
    let first = read_sheet_via_excel(bridge, full_path, &sheets[0])?;
    doc_sheets[0] = first;

    Ok(SheetDoc {
        file: name.to_string(),
        sheets: doc_sheets,
        active_sheet: 0,
        engine: "excel".to_string(),
        note: Some("Формулы считает установленный Excel (COM-автоматизация).".to_string()),
        dirty: false,
    })
}

/// Читает один лист из открытой книги Excel.
fn read_sheet_via_excel(
    bridge: &mut ExcelBridge,
    full_path: &Path,
    sheet_name: &str,
) -> Result<SheetGrid> {
    let data = bridge.read_sheet(&full_path.to_string_lossy(), sheet_name)?;
    let offset_row = data.row.saturating_sub(1);
    let offset_col = data.col.saturating_sub(1);
    let total_rows = offset_row + data.rows;
    let total_cols = offset_col + data.cols;
    if total_rows.saturating_mul(total_cols) > MAX_SHEET_CELLS {
        return Err(BioGitError::Excel("лист слишком велик для редактора".into()));
    }

    let mut grid = SheetGrid {
        name: sheet_name.to_string(),
        rows: Vec::with_capacity(total_rows),
    };
    for _ in 0..total_rows {
        grid.rows.push(
            (0..total_cols)
                .map(|_| SheetCell {
                    value: String::new(),
                    formula: None,
                })
                .collect(),
        );
    }
    for i in 0..data.rows {
        for j in 0..data.cols {
            let idx = i * data.cols + j;
            let cell = &mut grid.rows[offset_row + i][offset_col + j];
            cell.value = data.values.get(idx).cloned().unwrap_or_default();
            if let Some(f) = data.formulas.get(idx).and_then(|f| f.clone()) {
                if !f.is_empty() {
                    cell.formula = Some(if f.starts_with('=') {
                        f
                    } else {
                        format!("={f}")
                    });
                }
            }
        }
    }
    Ok(grid)
}

// ---------------------------------------------------------------------------
// Встроенный режим: calamine (чтение) + rust_xlsxwriter (запись)
// ---------------------------------------------------------------------------

fn open_builtin(full_path: &Path, name: &str) -> Result<SheetDoc> {
    use calamine::Reader;

    let mut book = calamine::open_workbook_auto(full_path).map_err(|e| {
        BioGitError::InvalidState(format!("не удалось открыть книгу: {e}"))
    })?;
    let sheet_names = book.sheet_names();
    if sheet_names.is_empty() {
        return Err(BioGitError::InvalidState("в книге нет листов".into()));
    }

    let mut grids = Vec::with_capacity(sheet_names.len());
    for sheet_name in &sheet_names {
        let range = book
            .worksheet_range(sheet_name)
            .map_err(|e| BioGitError::InvalidState(format!("лист «{sheet_name}»: {e}")))?;
        let (height, width) = range.get_size();
        if height.saturating_mul(width) > MAX_SHEET_CELLS {
            return Err(BioGitError::InvalidState(format!(
                "лист «{sheet_name}» слишком велик для встроенного режима (лимит {MAX_SHEET_CELLS} ячеек)"
            )));
        }

        let mut formulas: HashMap<(usize, usize), String> = HashMap::new();
        if let Ok(frange) = book.worksheet_formula(sheet_name) {
            // used_cells отдаёт координаты относительно начала диапазона
            let (fs_r, fs_c) = frange.start().unwrap_or((0, 0));
            let (fs_r, fs_c) = (fs_r as usize, fs_c as usize);
            for (r, c, text) in frange.used_cells() {
                if !text.is_empty() {
                    formulas.insert(
                        (fs_r + r, fs_c + c),
                        if text.starts_with('=') {
                            text.to_string()
                        } else {
                            format!("={text}")
                        },
                    );
                }
            }
        }

        // Сетка покрывает UsedRange с учётом его смещения от A1
        let (start_r, start_c) = range.start().unwrap_or((0, 0));
        let (start_r, start_c) = (start_r as usize, start_c as usize);
        let total_rows = start_r + height;
        let total_cols = start_c + width;
        if total_rows.saturating_mul(total_cols) > MAX_SHEET_CELLS {
            return Err(BioGitError::InvalidState(format!(
                "лист «{sheet_name}» слишком велик для встроенного режима (лимит {MAX_SHEET_CELLS} ячеек)"
            )));
        }
        let mut rows: Vec<Vec<SheetCell>> = Vec::with_capacity(total_rows);
        for _ in 0..total_rows {
            rows.push(
                (0..total_cols)
                    .map(|_| SheetCell {
                        value: String::new(),
                        formula: None,
                    })
                    .collect(),
            );
        }
        for (r, c, data) in range.used_cells() {
            let (ar, ac) = (start_r + r, start_c + c);
            if let Some(row) = rows.get_mut(ar) {
                if let Some(cell) = row.get_mut(ac) {
                    cell.value = display_data(data);
                }
            }
        }
        for ((r, c), f) in formulas {
            if let Some(row) = rows.get_mut(r) {
                if let Some(cell) = row.get_mut(c) {
                    cell.formula = Some(f);
                }
            }
        }

        grids.push(SheetGrid {
            name: sheet_name.clone(),
            rows,
        });
    }

    let ext = extension_of(name);
    let note = if ext == "xlsx" {
        Some(
            "Встроенный режим (Excel не найден): простые формулы считает примитивный движок \
             (+ − * / ^, SUM, AVERAGE, MIN, MAX, COUNT, ROUND, ABS, IF). Сохранение в .xlsx."
                .to_string(),
        )
    } else {
        Some(
            "Встроенный режим (Excel не найден): файл прочитан, но сохранение выполняется \
             в формат .xlsx."
                .to_string(),
        )
    };

    Ok(SheetDoc {
        file: name.to_string(),
        sheets: grids,
        active_sheet: 0,
        engine: "builtin".to_string(),
        note,
        dirty: false,
    })
}

fn display_data(data: &calamine::Data) -> String {
    use calamine::Data;
    match data {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => formula::format_number(*f),
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => {
            if *b {
                "TRUE".to_string()
            } else {
                "FALSE".to_string()
            }
        }
        Data::DateTime(dt) => match dt.as_datetime() {
            Some(d) => d.to_string(),
            None => formula::format_number(dt.as_f64()),
        },
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#{e:?}"),
    }
}

fn ensure_capacity(grid: &mut SheetGrid, rows: usize, cols: usize) {
    if grid.rows.len() < rows {
        grid.rows.resize_with(rows, Vec::new);
    }
    for row in &mut grid.rows {
        if row.len() < cols {
            row.resize_with(cols, || SheetCell {
                value: String::new(),
                formula: None,
            });
        }
    }
}

fn save_builtin(session: &SheetSession, repo: &Repo) -> Result<()> {
    use rust_xlsxwriter::{Formula, Workbook};

    let total: usize = session
        .doc
        .sheets
        .iter()
        .map(|g| g.rows.iter().map(|r| r.len()).sum::<usize>())
        .sum();
    if total > MAX_SHEET_CELLS {
        return Err(BioGitError::InvalidState(
            "книга слишком велика для сохранения во встроенном режиме".into(),
        ));
    }

    let target = repo.root().join(&session.doc.file);
    let mut workbook = Workbook::new();
    for grid in &session.doc.sheets {
        let ws = workbook.add_worksheet();
        ws.set_name(&grid.name)
            .map_err(|e| BioGitError::InvalidState(format!("имя листа: {e}")))?;
        for (r, row) in grid.rows.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let (r, c) = (r as u32, c as u16);
                if let Some(f) = &cell.formula {
                    // формула + кэшированный результат (Excel пересчитает при открытии)
                    let formula = Formula::new(f).set_result(&cell.value);
                    ws.write_formula(r, c, formula).map_err(|e| {
                        BioGitError::InvalidState(format!("запись формулы: {e}"))
                    })?;
                } else if !cell.value.is_empty() {
                    match parse_cached(&cell.value) {
                        CachedValue::Num(n) => {
                            ws.write_number(r, c, n).map_err(|e| {
                                BioGitError::InvalidState(format!("запись числа: {e}"))
                            })?;
                        }
                        CachedValue::Bool(b) => {
                            ws.write_boolean(r, c, b).map_err(|e| {
                                BioGitError::InvalidState(format!("запись логического: {e}"))
                            })?;
                        }
                        CachedValue::Text => {
                            ws.write_string(r, c, &cell.value).map_err(|e| {
                                BioGitError::InvalidState(format!("запись текста: {e}"))
                            })?;
                        }
                    }
                }
            }
        }
    }
    workbook
        .save(&target)
        .map_err(|e| BioGitError::InvalidState(format!("сохранение книги: {e}")))?;
    Ok(())
}

enum CachedValue {
    Num(f64),
    Bool(bool),
    Text,
}

fn parse_cached(value: &str) -> CachedValue {
    if value == "TRUE" {
        return CachedValue::Bool(true);
    }
    if value == "FALSE" {
        return CachedValue::Bool(false);
    }
    if let Ok(n) = value.replace(',', ".").parse::<f64>() {
        return CachedValue::Num(n);
    }
    CachedValue::Text
}

// ---------------------------------------------------------------------------
// Мост: JSON-протокол с PowerShell-скриптом COM-автоматизации
// ---------------------------------------------------------------------------

/// Данные чтения листа из Excel.
struct SheetReadData {
    /// 1-индексированные строка/колонка начала UsedRange.
    row: usize,
    col: usize,
    rows: usize,
    cols: usize,
    /// Значения построчно (размер rows*cols).
    values: Vec<String>,
    /// Формулы построчно (`None` — не формула).
    formulas: Vec<Option<String>>,
}

/// Мост к Excel: отдельный процесс PowerShell с COM-объектом Excel.Application.
struct ExcelBridge {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    rx: std::sync::mpsc::Receiver<String>,
    next_id: u64,
}

const EXCEL_BRIDGE_PS1: &str = include_str!("excel_bridge.ps1");

impl ExcelBridge {
    /// Отправляет операцию и ждёт ответ с тем же id.
    fn request(
        &mut self,
        op: &str,
        args: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value> {
        self.next_id += 1;
        let id = self.next_id;
        let req = serde_json::json!({"id": id, "op": op, "args": args});
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.flush()?;

        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(BioGitError::Excel(format!(
                    "превышено время ожидания операции «{op}» — возможно, Excel заблокирован диалогом"
                )));
            }
            let remain = deadline - now;
            match self.rx.recv_timeout(remain) {
                Ok(text) => {
                    let resp: serde_json::Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if resp.get("id").and_then(|v| v.as_u64()) != Some(id) {
                        continue; // ответ на устаревший запрос
                    }
                    if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                        return Ok(resp.get("data").cloned().unwrap_or(serde_json::Value::Null));
                    }
                    let message = resp
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("неизвестная ошибка Excel")
                        .to_string();
                    return Err(BioGitError::Excel(message));
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    return Err(BioGitError::Excel(format!(
                        "превышено время ожидания операции «{op}»"
                    )));
                }
                Err(_) => {
                    return Err(BioGitError::Excel(
                        "процесс Excel-моста неожиданно завершён".into(),
                    ));
                }
            }
        }
    }

    fn ping(&mut self) -> Result<String> {
        let data = self.request("ping", serde_json::json!({}), TIMEOUT_PING)?;
        Ok(data
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string())
    }

    fn open(&mut self, path: &str) -> Result<Vec<String>> {
        let data = self.request("open", serde_json::json!({"path": path}), TIMEOUT_OPEN)?;
        let sheets = data
            .get("sheets")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(sheets
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect())
    }

    fn read_sheet(&mut self, path: &str, sheet: &str) -> Result<SheetReadData> {
        let data = self.request(
            "read",
            serde_json::json!({"path": path, "sheet": sheet}),
            TIMEOUT_READ,
        )?;
        let num = |key: &str| data.get(key).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let values = json_string_list(data.get("values"));
        let formulas = json_opt_string_list(data.get("formulas"));
        Ok(SheetReadData {
            row: num("row"),
            col: num("col"),
            rows: num("rows"),
            cols: num("cols"),
            values,
            formulas,
        })
    }

    fn edit(
        &mut self,
        path: &Path,
        sheet: &str,
        row: usize,
        col: usize,
        input: &str,
    ) -> Result<(String, Option<String>)> {
        let data = self.request(
            "edit",
            serde_json::json!({
                "path": path.to_string_lossy(),
                "sheet": sheet,
                "row": row + 1, // COM нумерует с 1
                "col": col + 1,
                "input": input,
            }),
            TIMEOUT_EDIT,
        )?;
        let value = json_display(&data.get("value").cloned().unwrap_or(serde_json::Value::Null));
        let formula = data
            .get("formula")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|f| !f.is_empty());
        Ok((value, formula))
    }

    fn save(&mut self, path: &Path) -> Result<()> {
        self.request(
            "save",
            serde_json::json!({"path": path.to_string_lossy()}),
            TIMEOUT_SAVE,
        )?;
        Ok(())
    }

    fn close_workbook(&mut self, path: &Path) -> Result<()> {
        self.request(
            "close",
            serde_json::json!({"path": path.to_string_lossy()}),
            TIMEOUT_CLOSE,
        )?;
        Ok(())
    }
}

impl Drop for ExcelBridge {
    fn drop(&mut self) {
        // стараемся корректно закрыть Excel, затем прибиваем процесс
        let _ = self.request("quit", serde_json::json!({}), Duration::from_secs(5));
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(windows)]
fn spawn_bridge() -> Result<ExcelBridge> {
    use std::io::BufRead;
    use std::process::{Command, Stdio};

    let script_path = std::env::temp_dir().join("biogit_excel_bridge.ps1");
    std::fs::write(&script_path, EXCEL_BRIDGE_PS1)?;

    let mut child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| BioGitError::Excel("stdin моста недоступен".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| BioGitError::Excel("stdout моста недоступен".into()))?;

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::Builder::new()
        .name("biogit-excel-bridge".to_string())
        .spawn(move || {
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(text) => {
                        if tx.send(text).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })?;

    Ok(ExcelBridge {
        child,
        stdin,
        rx,
        next_id: 0,
    })
}

#[cfg(not(windows))]
fn spawn_bridge() -> Result<ExcelBridge> {
    Err(BioGitError::Excel(
        "COM-автоматизация Excel доступна только на Windows".into(),
    ))
}

/// Проверяет доступность Excel (запускает мост и пингует).
pub fn excel_probe() -> ExcelAvailability {
    #[cfg(not(windows))]
    {
        return ExcelAvailability {
            available: false,
            reason: Some("COM-автоматизация Excel доступна только на Windows".to_string()),
            version: None,
        };
    }
    #[cfg(windows)]
    {
        match spawn_bridge() {
            Ok(mut bridge) => match bridge.ping() {
                Ok(version) => ExcelAvailability {
                    available: true,
                    reason: None,
                    version: Some(version),
                },
                Err(e) => ExcelAvailability {
                    available: false,
                    reason: Some(e.to_string()),
                    version: None,
                },
            },
            Err(e) => ExcelAvailability {
                available: false,
                reason: Some(e.to_string()),
                version: None,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// JSON-хелперы
// ---------------------------------------------------------------------------

fn json_display(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => {
            if *b {
                "TRUE".to_string()
            } else {
                "FALSE".to_string()
            }
        }
        serde_json::Value::Number(n) => match n.as_f64() {
            Some(f) => formula::format_number(f),
            None => n.to_string(),
        },
        other => other.to_string(),
    }
}

fn json_string_list(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(json_display).collect())
        .unwrap_or_default()
}

fn json_opt_string_list(value: Option<&serde_json::Value>) -> Vec<Option<String>> {
    value
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|item| match item {
                    serde_json::Value::String(s) => Some(s.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
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

    /// Создаёт книгу с числами, формулой и текстом через rust_xlsxwriter.
    fn make_sample(path: &Path) {
        use rust_xlsxwriter::{Formula, Workbook};
        let mut wb = Workbook::new();
        let ws = wb.add_worksheet();
        ws.set_name("Данные").unwrap();
        ws.write_number(0, 0, 10.0).unwrap(); // A1
        ws.write_number(0, 1, 20.0).unwrap(); // B1
        ws.write_formula(0, 2, Formula::new("=A1+B1").set_result("30")).unwrap(); // C1
        ws.write_string(1, 0, "итог").unwrap();
        wb.save(path).unwrap();
    }

    #[test]
    fn встроенный_режим_открытие_правка_сохранение() {
        let (dir, repo) = repo_in_tmp();
        let file_path = dir.path().join("expr.xlsx");
        make_sample(&file_path);
        std::fs::write(repo.root().join("expr.xlsx"), std::fs::read(&file_path).unwrap()).unwrap();

        // открытие: Excel недоступен в тестовой среде — builtin
        let mut session = sheet_open(&repo, "expr.xlsx").expect("открытие");
        // если в CI есть Excel — пропускаем builtin-проверки (они покрыты кодом выше)
        if session.doc.engine != "builtin" {
            return;
        }
        assert_eq!(session.doc.sheets.len(), 1);
        assert_eq!(session.doc.sheets[0].name, "Данные");

        let grid = &session.doc.sheets[0];
        assert_eq!(grid.rows[0][0].value, "10");
        assert_eq!(grid.rows[0][1].value, "20");
        // формула прочитана, значение — кэш Excel
        assert_eq!(grid.rows[0][2].formula.as_deref(), Some("=A1+B1"));
        assert_eq!(grid.rows[0][2].value, "30");
        assert_eq!(grid.rows[1][0].value, "итог");

        // правка формулы: A1*2+B1 = 40
        let outcome = sheet_edit(&mut session, 0, 0, 2, "=A1*2+B1").expect("правка");
        assert_eq!(outcome.formula.as_deref(), Some("=A1*2+B1"));
        assert_eq!(outcome.value, "40", "примитивный движок пересчитал формулу");

        // литеральное значение
        let outcome = sheet_edit(&mut session, 0, 0, 1, "новый текст").expect("правка");
        assert_eq!(outcome.value, "новый текст");
        assert!(outcome.formula.is_none());

        assert!(sheet_dirty(&session));

        // сохранение
        let saved = sheet_save(&mut session, &repo).expect("сохранение");
        assert_eq!(saved, "expr.xlsx");

        // перечитывание: правки на диске
        let reloaded = sheet_open(&repo, "expr.xlsx").expect("перечитывание");
        if reloaded.doc.engine != "builtin" {
            return;
        }
        let grid = &reloaded.doc.sheets[0];
        assert_eq!(grid.rows[0][1].value, "новый текст");
        assert_eq!(grid.rows[0][2].formula.as_deref(), Some("=A1*2+B1"));
    }

    #[test]
    fn встроенный_режим_не_открывает_не_таблицы() {
        let (_dir, repo) = repo_in_tmp();
        std::fs::write(repo.root().join("notes.txt"), "текст").unwrap();
        let is_err = sheet_open(&repo, "notes.txt").is_err();
        assert!(is_err, "не-табличный файл должен быть отклонён");
    }

    #[test]
    fn очистка_и_пустой_ввод() {
        let (dir, repo) = repo_in_tmp();
        let file_path = dir.path().join("sample.xlsx");
        make_sample(&file_path);
        std::fs::write(repo.root().join("sample.xlsx"), std::fs::read(&file_path).unwrap()).unwrap();

        let mut session = sheet_open(&repo, "sample.xlsx").expect("открытие");
        if session.doc.engine != "builtin" {
            return;
        }
        let outcome = sheet_edit(&mut session, 0, 1, 0, "").expect("очистка");
        assert_eq!(outcome.value, "");
        assert!(outcome.formula.is_none());
    }
}

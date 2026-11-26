import { useCallback, useEffect, useState } from "react";
import {
  api,
  colName,
  type SheetDoc,
} from "../api";

// Редактор электронных таблиц (xlsx/xls/xlsm/xlsb/ods).
//
// Уровень A «Excel как вычислительный движок»: ячейки редактируются в
// интерфейсе BioGit, формулы считает настоящий Excel через COM-автоматизацию.
// Если Excel на машине нет — бэкенд автоматически переключается во встроенный
// режим, где простые формулы вычисляет примитивный движок ядра.

interface Props {
  path: string;
  name: string;
  dirty: boolean;
  busy: boolean;
  onSave: () => void;
  onDirtyChange: (dirty: boolean) => void;
}

const MAX_RENDER_CELLS = 20000;

function engineBadge(engine: SheetDoc["engine"]): { label: string; cls: string } {
  if (engine === "excel") {
    return { label: "Excel (COM)", cls: "badge badge-excel" };
  }
  return { label: "встроенный режим", cls: "badge badge-builtin" };
}

export default function SheetEditor({ path, name, dirty, busy, onSave, onDirtyChange }: Props) {
  const [doc, setDoc] = useState<SheetDoc | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<{ r: number; c: number } | null>(null);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);

  const open = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const opened = await api.sheetOpen(path, name);
      setDoc(opened);
      setSelected(null);
      setDraft("");
      onDirtyChange(opened.dirty);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [path, name, onDirtyChange]);

  useEffect(() => {
    void open();
    // сессия Excel остаётся открытой в бэкенде при переключении вкладок —
    // закрытие происходит при закрытии вкладки или открытии другой книги
  }, [open]);

  const activeGrid = doc?.sheets[doc.active_sheet];
  const cellValue = (r: number, c: number): string =>
    activeGrid?.rows[r]?.[c]?.value ?? "";
  const cellFormula = (r: number, c: number): string | null =>
    activeGrid?.rows[r]?.[c]?.formula ?? null;

  const selectCell = (r: number, c: number) => {
    setSelected({ r, c });
    setDraft(cellFormula(r, c) ?? cellValue(r, c));
  };

  const commitEdit = async () => {
    if (!doc || !selected) return;
    try {
      const outcome = await api.sheetEdit(doc.active_sheet, selected.r, selected.c, draft);
      setDoc((prev) => {
        if (!prev) return prev;
        const sheets = prev.sheets.map((grid, i) => {
          if (i !== prev.active_sheet) return grid;
          const rows = grid.rows.map((row) => row.map((cell) => ({ ...cell })));
          while (rows.length <= selected.r) rows.push([]);
          while (rows[selected.r].length <= selected.c) {
            rows[selected.r].push({ value: "", formula: null });
          }
          rows[selected.r][selected.c] = {
            value: outcome.value,
            formula: outcome.formula,
          };
          return { ...grid, rows };
        });
        return { ...prev, sheets, dirty: true };
      });
      onDirtyChange(true);
    } catch (e) {
      setError(String(e));
    }
  };

  const switchSheet = async (index: number) => {
    if (!doc) return;
    try {
      await api.sheetSetActive(index);
      setDoc((prev) => (prev ? { ...prev, active_sheet: index } : prev));
      setSelected(null);
      setDraft("");
    } catch (e) {
      setError(String(e));
    }
  };

  const handleSave = async () => {
    setSaving(true);
    setError(null);
    try {
      const savedName = await api.sheetSave(path);
      setDoc((prev) => (prev ? { ...prev, dirty: false, file: savedName } : prev));
      onDirtyChange(false);
      onSave();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  if (loading) {
    return <div className="alert">Открываем таблицу…</div>;
  }
  if (error && !doc) {
    return <div className="alert alert-error">{error}</div>;
  }
  if (!doc || !activeGrid) {
    return <div className="alert alert-error">Не удалось открыть книгу</div>;
  }

  const badge = engineBadge(doc.engine);
  const totalCells = activeGrid.rows.reduce((sum, row) => sum + row.length, 0);
  const truncated = totalCells > MAX_RENDER_CELLS;
  const renderRows = truncated ? Math.floor(MAX_RENDER_CELLS / Math.max(activeGrid.rows[0]?.length ?? 1, 1)) : activeGrid.rows.length;
  const renderCols = activeGrid.rows[0]?.length ?? 0;

  return (
    <div className="editor">
      <div className="editor-head">
        <span className="mono">{name}</span>
        {dirty && <span className="badge badge-unsaved">не сохранено</span>}
        <span className={badge.cls} title={doc.note ?? undefined}>{badge.label}</span>
        <span className="editor-head-spacer" />
        <button
          type="button"
          className="btn btn-primary btn-sm"
          onClick={() => void handleSave()}
          disabled={busy || saving || !dirty}
          title="Сохранить книгу (Ctrl+S)"
        >
          Сохранить файл
        </button>
      </div>

      {doc.sheets.length > 1 && (
        <div className="sheet-tabs">
          {doc.sheets.map((sheet, i) => (
            <button
              key={sheet.name}
              type="button"
              className={`sheet-tab ${i === doc.active_sheet ? "sheet-tab-active" : ""}`}
              onClick={() => void switchSheet(i)}
            >
              {sheet.name}
            </button>
          ))}
        </div>
      )}

      <div className="formula-bar">
        <span className="formula-cell-ref">
          {selected ? `${colName(selected.c)}${selected.r + 1}` : "—"}
        </span>
        <input
          className="input formula-input"
          placeholder="Значение или формула (=SUM(A1:A10))"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void commitEdit();
            }
          }}
          disabled={!selected}
        />
        <button
          type="button"
          className="btn btn-ghost btn-sm"
          onClick={() => void commitEdit()}
          disabled={!selected}
        >
          Ввод
        </button>
      </div>

      <div className="sheet-grid-wrap">
        <table className="sheet-grid">
          <thead>
            <tr>
              <th className="sheet-rownum" />
              {Array.from({ length: renderCols }, (_, c) => (
                <th key={c} className="sheet-colhead">{colName(c)}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {Array.from({ length: renderRows }, (_, r) => (
              <tr key={r}>
                <td className="sheet-rownum">{r + 1}</td>
                {Array.from({ length: renderCols }, (_, c) => {
                  const isSelected = selected?.r === r && selected?.c === c;
                  const isFormula = cellFormula(r, c) !== null;
                  return (
                    <td
                      key={c}
                      className={`sheet-cell ${isSelected ? "sheet-cell-selected" : ""} ${isFormula ? "sheet-cell-formula" : ""}`}
                      onClick={() => selectCell(r, c)}
                    >
                      {cellValue(r, c)}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
        {truncated && (
          <div className="alert">
            Показаны первые {MAX_RENDER_CELLS.toLocaleString("ru-RU")} ячеек — лист слишком велик для отображения.
          </div>
        )}
      </div>

      {doc.note && <p className="muted small sheet-note">{doc.note}</p>}
      {error && <div className="alert alert-error">{error}</div>}
      <div className="editor-status">
        <span>{doc.engine === "excel" ? "движок: Microsoft Excel" : "движок: встроенный (примитив)"}</span>
        <span>{activeGrid.rows.length} строк</span>
        <span className="editor-status-spacer" />
        <span className="muted">
          {dirty ? "есть изменения — Ctrl+S, чтобы сохранить" : "сохранено"}
        </span>
      </div>
    </div>
  );
}

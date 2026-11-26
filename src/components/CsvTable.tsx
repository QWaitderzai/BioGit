import { useMemo, useState } from "react";

// Табличное отображение CSV/TSV в стиле программы: клик по ячейке — правка,
// правки синхронизируются с текстом файла (модель текста остаётся источником истины).

function parseCsv(text: string, delimiter: string): string[][] {
  // Поддержка кавычек с переводами строк внутри полей
  const rows: string[][] = [];
  let field = "";
  let inQuotes = false;
  let row: string[] = [];
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (inQuotes) {
      if (ch === '"') {
        if (text[i + 1] === '"') {
          field += '"';
          i++;
        } else {
          inQuotes = false;
        }
      } else {
        field += ch;
      }
    } else if (ch === '"') {
      inQuotes = true;
    } else if (ch === delimiter) {
      row.push(field);
      field = "";
    } else if (ch === "\n") {
      row.push(field);
      field = "";
      rows.push(row);
      row = [];
    } else if (ch === "\r") {
      // пропускаем (\n обработает перевод)
    } else {
      field += ch;
    }
  }
  if (field !== "" || row.length > 0) {
    row.push(field);
    rows.push(row);
  }
  return rows;
}

function serializeCsv(rows: string[][], delimiter: string): string {
  const escape = (field: string) => {
    if (
      field.includes(delimiter) ||
      field.includes('"') ||
      field.includes("\n") ||
      field.includes("\r")
    ) {
      return `"${field.replace(/"/g, '""')}"`;
    }
    return field;
  };
  return rows.map((row) => row.map(escape).join(delimiter)).join("\n") + "\n";
}

interface Props {
  text: string;
  onChange: (text: string) => void;
  readOnly?: boolean;
}

export default function CsvTable({ text, onChange, readOnly }: Props) {
  const delimiter = useMemo(() => (text.includes(";") && !text.includes(",")) ? ";" : text.includes("\t") ? "\t" : ",", [text]);
  const rows = useMemo(() => parseCsv(text, delimiter), [text, delimiter]);
  const [editing, setEditing] = useState<{ r: number; c: number } | null>(null);
  const [draft, setDraft] = useState("");

  const startEdit = (r: number, c: number) => {
    if (readOnly) return;
    setEditing({ r, c });
    setDraft(rows[r]?.[c] ?? "");
  };

  const commit = () => {
    if (!editing) return;
    const next = rows.map((row) => [...row]);
    while (next.length <= editing.r) next.push([]);
    while (next[editing.r].length <= editing.c) next[editing.r].push("");
    next[editing.r][editing.c] = draft;
    onChange(serializeCsv(next, delimiter));
    setEditing(null);
  };

  const width = rows.reduce((max, row) => Math.max(max, row.length), 0);
  if (width === 0) {
    return <p className="empty">Пустая таблица</p>;
  }

  return (
    <div className="csv-wrap">
      <table className="csv-table">
        <thead>
          <tr>
            <th className="csv-rownum">#</th>
            {Array.from({ length: width }, (_, c) => (
              <th key={c}>{c}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, r) => (
            <tr key={r}>
              <td className="csv-rownum">{r + 1}</td>
              {Array.from({ length: width }, (_, c) => {
                const isEditing = editing?.r === r && editing?.c === c;
                return (
                  <td
                    key={c}
                    className={isEditing ? "csv-cell-editing" : undefined}
                    onClick={() => startEdit(r, c)}
                  >
                    {isEditing ? (
                      <input
                        className="csv-cell-input"
                        value={draft}
                        autoFocus
                        onChange={(e) => setDraft(e.target.value)}
                        onBlur={commit}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") commit();
                          if (e.key === "Escape") setEditing(null);
                        }}
                      />
                    ) : (
                      row[c] ?? ""
                    )}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
      {!readOnly && (
        <p className="muted small csv-hint">
          Клик по ячейке — правка. Enter — сохранить значение, Esc — отмена.
        </p>
      )}
    </div>
  );
}

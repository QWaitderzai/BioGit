import { formatSize, type CsvDiff, type FileDiff, type DiffLine } from "../api";

// Просмотр отличий: построчный diff текста и табличный diff CSV,
// в стиле всего приложения (зелёное добавлено, красное удалено).

function stateLabel(state: FileDiff["state"]): string {
  if (state === "added") return "новый";
  if (state === "deleted") return "удалён";
  return "изменён";
}

function LineRow({ line }: { line: DiffLine }) {
  const cls =
    line.kind === "plus"
      ? "diff-line diff-line-plus"
      : line.kind === "minus"
        ? "diff-line diff-line-minus"
        : "diff-line";
  const sign = line.kind === "plus" ? "+" : line.kind === "minus" ? "−" : " ";
  return (
    <div className={cls}>
      <span className="diff-sign">{sign}</span>
      <span className="diff-text">{line.text.replace(/\n$/, "") || " "}</span>
    </div>
  );
}

function CsvDiffView({ csv, state }: { csv: CsvDiff; state: FileDiff["state"] }) {
  const hasChanges =
    csv.added.length > 0 || csv.removed.length > 0 || csv.changed.length > 0;
  if (!hasChanges) {
    return <p className="muted">Изменений в таблице нет</p>;
  }
  return (
    <div className="csv-diff">
      {csv.changed.length > 0 && (
        <>
          <div className="detail-label">Изменённые ячейки ({csv.changed.length})</div>
          <table className="csv-table csv-diff-table">
            <thead>
              <tr>
                {csv.header.map((h) => (
                  <th key={h}>{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {csv.changed.map((cell, i) => (
                <tr key={`${cell.key}-${i}`}>
                  {csv.header.map((h) => {
                    if (h === csv.header[0]) {
                      return <td key={h}>{cell.key}</td>;
                    }
                    if (h === cell.column) {
                      return (
                        <td key={h} className="csv-cell-changed">
                          <span className="diff-old">{cell.old || "∅"}</span>
                          <span className="diff-arrow">→</span>
                          <span className="diff-new">{cell.new || "∅"}</span>
                        </td>
                      );
                    }
                    return <td key={h} className="muted">—</td>;
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
      {csv.added.length > 0 && (
        <>
          <div className="detail-label">Добавленные строки ({csv.added.length})</div>
          <table className="csv-table csv-diff-table">
            <thead>
              <tr>
                {csv.header.map((h) => (
                  <th key={h}>{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {csv.added.map((row, i) => (
                <tr key={i} className="csv-row-added">
                  {csv.header.map((h, c) => (
                    <td key={h}>{row[c] ?? ""}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
      {csv.removed.length > 0 && (
        <>
          <div className="detail-label">Удалённые строки ({csv.removed.length})</div>
          <table className="csv-table csv-diff-table">
            <thead>
              <tr>
                {csv.header.map((h) => (
                  <th key={h}>{h}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {csv.removed.map((row, i) => (
                <tr key={i} className="csv-row-removed">
                  {csv.header.map((h, c) => (
                    <td key={h}>{row[c] ?? ""}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
      {state === "deleted" && <p className="muted">Файл удалён целиком</p>}
    </div>
  );
}

function FileDiffCard({ diff }: { diff: FileDiff }) {
  return (
    <section className="diff-file">
      <div className="diff-file-head">
        <span className="mono">{diff.path}</span>
        <span className={`badge st-${diff.state}`}>{stateLabel(diff.state)}</span>
        <span className="muted small">
          {diff.old_size !== null && `было ${formatSize(diff.old_size)} · `}
          {diff.new_size !== null && `стало ${formatSize(diff.new_size)}`}
          {diff.kind === "binary" && " · двоичный"}
        </span>
      </div>
      {diff.kind === "binary" ? (
        <p className="muted">
          Двоичный файл — содержимое не показывается. Сравните версии, открыв их
          во внешней программе.
        </p>
      ) : diff.kind === "csv" && diff.csv ? (
        <CsvDiffView csv={diff.csv} state={diff.state} />
      ) : diff.lines ? (
        <div className="diff-lines">
          {diff.lines.map((line, i) => (
            <LineRow key={i} line={line} />
          ))}
        </div>
      ) : (
        <p className="muted">Нет отличий для отображения</p>
      )}
    </section>
  );
}

export default function DiffView({ diffs }: { diffs: FileDiff[] }) {
  if (diffs.length === 0) {
    return <p className="empty">Различий нет — состояния совпадают</p>;
  }
  return (
    <div className="diff-view">
      {diffs.map((d) => (
        <FileDiffCard key={d.path} diff={d} />
      ))}
    </div>
  );
}

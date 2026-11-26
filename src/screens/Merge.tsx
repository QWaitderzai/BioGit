import { useCallback, useEffect, useMemo, useState } from "react";
import {
  api,
  proposalCleanChanges,
  proposalHasConflicts,
  shortHash,
  type ConflictDetail,
  type FileDecision,
  type MergeProposal,
  type MergeReport,
  type RepoState,
} from "../api";
import { useConfirm } from "../components/Modal";

interface Props {
  path: string;
  flash: (message: string) => void;
}

const SIDE_OURS = "ours";
const SIDE_THEIRS = "theirs";
const CELL_SEP = "\u001f";

function cellKey(rowKey: string, column: string): string {
  return `${rowKey}${CELL_SEP}${column}`;
}

/** Считает, сколько конфликтов уже разрешено и сколько всего. */
function countResolution(
  proposal: MergeProposal,
  decisions: Map<string, FileDecision>,
): { resolved: number; total: number } {
  let resolved = 0;
  let total = 0;
  for (const file of proposal.files) {
    if (file.status !== "conflict") continue;
    const d = decisions.get(file.path);
    for (const conflict of file.conflicts) {
      total += 1;
      const side = (s: string | undefined) => s === SIDE_OURS || s === SIDE_THEIRS;
      if (conflict.type === "text_hunk") {
        if (d && side(d.hunks[String(conflict.index)])) resolved += 1;
      } else if (conflict.type === "csv_cell") {
        if (d && side(d.cells[cellKey(conflict.row_key, conflict.column)])) resolved += 1;
      } else if (conflict.type === "csv_row") {
        if (d && side(d.rows[conflict.row_key])) resolved += 1;
      } else if (d && d.action !== "merged") {
        resolved += 1;
      }
    }
  }
  return { resolved, total };
}

function SideButton({
  side,
  active,
  onClick,
}: {
  side: "ours" | "theirs";
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`btn btn-sm ${active ? (side === "ours" ? "btn-primary" : "btn-theirs-active") : "btn-ghost"}`}
      onClick={onClick}
    >
      {side === "ours" ? "Наш вариант" : "Их вариант"}
    </button>
  );
}

/** Карточка разрешения конфликтов одного файла. */
function ConflictFile({
  file,
  decision,
  onChange,
}: {
  file: MergeProposal["files"][number];
  decision: FileDecision | undefined;
  onChange: (next: FileDecision) => void;
}) {
  const current: FileDecision =
    decision ?? {
      path: file.path,
      action: "merged",
      hunks: {},
      cells: {},
      rows: {},
    };

  const applyAll = (side: "ours" | "theirs") => {
    const hunks: Record<string, string> = {};
    const cells: Record<string, string> = {};
    const rows: Record<string, string> = {};
    let hasFileLevel = false;
    for (const conflict of file.conflicts) {
      if (conflict.type === "text_hunk") hunks[String(conflict.index)] = side;
      else if (conflict.type === "csv_cell") cells[cellKey(conflict.row_key, conflict.column)] = side;
      else if (conflict.type === "csv_row") rows[conflict.row_key] = side;
      else hasFileLevel = true;
    }
    onChange({
      path: file.path,
      action: hasFileLevel ? side : "merged",
      hunks,
      cells,
      rows,
    });
  };

  const sideFor = (conflict: ConflictDetail): "ours" | "theirs" | null => {
    const d = decision;
    if (!d) return null;
    const side = (s: string | undefined) =>
      s === SIDE_OURS ? "ours" : s === SIDE_THEIRS ? "theirs" : null;
    if (conflict.type === "text_hunk") return side(d.hunks[String(conflict.index)]);
    if (conflict.type === "csv_cell")
      return side(d.cells[cellKey(conflict.row_key, conflict.column)]);
    if (conflict.type === "csv_row") return side(d.rows[conflict.row_key]);
    return d.action === "ours" ? "ours" : d.action === "theirs" ? "theirs" : null;
  };

  return (
    <section className="conflict-file">
      <div className="conflict-file-head">
        <span className="mono">{file.path}</span>
        <span className="badge badge-merge">{file.kind}</span>
        <span className="conflict-file-spacer" />
        <button type="button" className="btn btn-ghost btn-sm" onClick={() => applyAll(SIDE_OURS)}>
          Всё наше
        </button>
        <button type="button" className="btn btn-ghost btn-sm" onClick={() => applyAll(SIDE_THEIRS)}>
          Всё ихнее
        </button>
      </div>

      {file.conflicts.map((conflict, ci) => {
        const chosen = sideFor(conflict);
        if (conflict.type === "text_hunk") {
          return (
            <div key={ci} className="conflict-hunk">
              <div className="conflict-hunk-head">
                <span className="muted small">Блок {conflict.index + 1}</span>
                <span className="conflict-hunk-spacer" />
                <SideButton side="ours" active={chosen === "ours"} onClick={() =>
                  onChange({ ...current, hunks: { ...current.hunks, [String(conflict.index)]: SIDE_OURS } })
                } />
                <SideButton side="theirs" active={chosen === "theirs"} onClick={() =>
                  onChange({ ...current, hunks: { ...current.hunks, [String(conflict.index)]: SIDE_THEIRS } })
                } />
              </div>
              <div className="conflict-sides">
                <pre className={`conflict-side conflict-side-ours ${chosen === "ours" ? "conflict-side-chosen" : ""}`}>
                  {conflict.ours.join("") || "(пусто)"}
                </pre>
                <pre className={`conflict-side conflict-side-theirs ${chosen === "theirs" ? "conflict-side-chosen" : ""}`}>
                  {conflict.theirs.join("") || "(пусто)"}
                </pre>
              </div>
            </div>
          );
        }
        if (conflict.type === "csv_cell") {
          return (
            <div key={ci} className="conflict-cell">
              <span className="mono">{conflict.row_key}</span> · колонка{" "}
              <span className="mono">{conflict.column}</span>
              <span className="conflict-values">
                <span className="diff-old">{conflict.ours || "∅"}</span>
                <span className="diff-arrow">→</span>
                <span className="diff-new">{conflict.theirs || "∅"}</span>
              </span>
              <span className="conflict-cell-spacer" />
              <SideButton side="ours" active={chosen === "ours"} onClick={() =>
                onChange({ ...current, cells: { ...current.cells, [cellKey(conflict.row_key, conflict.column)]: SIDE_OURS } })
              } />
              <SideButton side="theirs" active={chosen === "theirs"} onClick={() =>
                onChange({ ...current, cells: { ...current.cells, [cellKey(conflict.row_key, conflict.column)]: SIDE_THEIRS } })
              } />
            </div>
          );
        }
        if (conflict.type === "csv_row") {
          return (
            <div key={ci} className="conflict-hunk">
              <div className="conflict-hunk-head">
                <span className="muted small">
                  Строка «{conflict.row_key}» —{" "}
                  {conflict.ours === null
                    ? "удалена у нас, изменена у них"
                    : conflict.theirs === null
                      ? "изменена у нас, удалена у них"
                      : "добавлена в обеих ветках по-разному"}
                </span>
                <span className="conflict-hunk-spacer" />
                <SideButton side="ours" active={chosen === "ours"} onClick={() =>
                  onChange({ ...current, rows: { ...current.rows, [conflict.row_key]: SIDE_OURS } })
                } />
                <SideButton side="theirs" active={chosen === "theirs"} onClick={() =>
                  onChange({ ...current, rows: { ...current.rows, [conflict.row_key]: SIDE_THEIRS } })
                } />
              </div>
              <div className="conflict-sides">
                <pre className={`conflict-side conflict-side-ours ${chosen === "ours" ? "conflict-side-chosen" : ""}`}>
                  {conflict.ours ? conflict.ours.join(", ") : "(строка удалена)"}
                </pre>
                <pre className={`conflict-side conflict-side-theirs ${chosen === "theirs" ? "conflict-side-chosen" : ""}`}>
                  {conflict.theirs ? conflict.theirs.join(", ") : "(строка удалена)"}
                </pre>
              </div>
            </div>
          );
        }
        // file-level
        return (
          <div key={ci} className="conflict-file-level">
            <span className="alert alert-error-inline">{conflict.reason}</span>
            <span className="conflict-cell-spacer" />
            <SideButton side="ours" active={chosen === "ours"} onClick={() =>
              onChange({ ...current, action: SIDE_OURS })
            } />
            <SideButton side="theirs" active={chosen === "theirs"} onClick={() =>
              onChange({ ...current, action: SIDE_THEIRS })
            } />
          </div>
        );
      })}
    </section>
  );
}

export default function MergeScreen({ path, flash }: Props) {
  const confirm = useConfirm();
  const [state, setState] = useState<RepoState | null>(null);
  const [source, setSource] = useState("");
  const [newBranch, setNewBranch] = useState("");
  const [csvKey, setCsvKey] = useState("");
  const [proposal, setProposal] = useState<MergeProposal | null>(null);
  const [decisions, setDecisions] = useState<Map<string, FileDecision>>(new Map());
  const [report, setReport] = useState<MergeReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const opened: RepoState = await api.openRepo(path);
      setState(opened);
    } catch (e) {
      setError(String(e));
    }
  }, [path]);

  useEffect(() => {
    void reload();
    setProposal(null);
    setDecisions(new Map());
    setReport(null);
  }, [reload]);

  const otherBranches = (state?.branches ?? [])
    .map((b) => b.name)
    .filter((name) => name !== state?.current_branch);

  const handleCreateBranch = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.createBranch(path, newBranch.trim());
      flash(`Ветвь гипотезы «${newBranch.trim()}» создана`);
      setNewBranch("");
      await reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleSwitch = async (name: string) => {
    const ok = await confirm({
      title: "Переключиться на ветку?",
      message: (
        <>
          Файлы рабочей папки будут заменены содержимым кончика ветки{" "}
          <code>{name}</code>. Несохранённые правки останутся в файлах, которых
          в снимке нет.
        </>
      ),
      confirmLabel: "Переключиться",
    });
    if (!ok) return;
    setBusy(true);
    setError(null);
    setReport(null);
    try {
      await api.switchBranch(path, name);
      flash(`Переключились на ветку «${name}», файлы восстановлены`);
      await reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // Фаза 1: предложение слияния
  const handlePropose = async () => {
    if (!source) return;
    setBusy(true);
    setError(null);
    setReport(null);
    try {
      const key = csvKey.trim() || undefined;
      const result = await api.mergePropose(path, source, key);
      setProposal(result);
      setDecisions(new Map());
      if (!proposalHasConflicts(result)) {
        // чистое слияние — подтверждаем и применяем сразу
        const changes = proposalCleanChanges(result);
        const ok = await confirm({
          title: "Слияние без конфликтов",
          message: (
            <>
              Изменения будут применены к файлам:{" "}
              {changes.length > 0 ? (
                <ul className="merge-list">
                  {changes.map((f) => (
                    <li key={f} className="merge-ok">{f}</li>
                  ))}
                </ul>
              ) : (
                "изменений нет"
              )}
            </>
          ),
          confirmLabel: "Применить",
        });
        if (!ok) {
          setProposal(null);
          return;
        }
        const report = await api.mergeApply(path, source, key ?? null, []);
        setReport(report);
        setProposal(null);
        if (report.snapshot) {
          flash(`Слияние выполнено: ${shortHash(report.snapshot.hash)}`);
        }
        await reload();
      } else {
        flash(
          `Найдено конфликтов: ${countResolution(result, new Map()).total} — разрешите их ниже`,
        );
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const conflictingFiles = useMemo(
    () => proposal?.files.filter((f) => f.status === "conflict") ?? [],
    [proposal],
  );
  const { resolved, total } = useMemo(
    () => (proposal ? countResolution(proposal, decisions) : { resolved: 0, total: 0 }),
    [proposal, decisions],
  );

  // Фаза 2: применить с решениями
  const handleApply = async () => {
    if (!proposal) return;
    const key = csvKey.trim() || null;
    const decisionList = [...decisions.values()];
    setBusy(true);
    setError(null);
    try {
      const result = await api.mergeApply(path, proposal.source, key, decisionList);
      setReport(result);
      setProposal(null);
      setDecisions(new Map());
      if (result.snapshot) {
        flash(`Слияние выполнено: ${shortHash(result.snapshot.hash)}`);
      }
      await reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const cancelProposal = () => {
    setProposal(null);
    setDecisions(new Map());
  };

  if (!state) {
    return <div className="alert alert-error">{error ?? "Загрузка…"}</div>;
  }

  return (
    <div className="merge">
      <section className="panel">
        <h2 className="panel-title">Ветви гипотез</h2>
        <div className="branch-list">
          {state.branches.map((b) => (
            <div key={b.name} className="branch-row">
              <div>
                <span className="badge badge-branch">{b.name}</span>
                {b.name === state.current_branch && (
                  <span className="muted"> — текущая</span>
                )}
                <div className="muted small">
                  {b.latest_message ?? "нет снимков"}
                </div>
              </div>
              {b.name !== state.current_branch && (
                <button
                  className="btn btn-ghost btn-sm"
                  onClick={() => void handleSwitch(b.name)}
                  disabled={busy}
                >
                  Переключиться
                </button>
              )}
            </div>
          ))}
        </div>
        <div className="save-bar">
          <input
            className="input input-grow"
            placeholder="Имя новой ветви"
            value={newBranch}
            onChange={(e) => setNewBranch(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleCreateBranch()}
          />
          <button
            className="btn btn-ghost"
            onClick={handleCreateBranch}
            disabled={busy || !newBranch.trim()}
          >
            Создать ветвь
          </button>
        </div>
      </section>

      <section className="panel">
        <h2 className="panel-title">Слияние</h2>
        <p className="panel-hint">
          Ветка-источник вливается в текущую ветку{" "}
          <span className="badge badge-branch">{state.current_branch}</span>.
          Сначала показывается предложение слияния; конфликты разрешаются
          по блокам текста, ячейкам и строкам CSV — прямо здесь.
        </p>
        <div className="save-bar">
          <input
            className="input"
            placeholder="Ключевая колонка CSV (по умолчанию первая)"
            value={csvKey}
            onChange={(e) => setCsvKey(e.target.value)}
            title="Например: gene_id"
          />
          <select
            className="input select select-grow"
            value={source}
            onChange={(e) => setSource(e.target.value)}
          >
            <option value="">— выберите ветку-источник —</option>
            {otherBranches.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
          <button
            className="btn btn-primary"
            onClick={handlePropose}
            disabled={busy || !source}
          >
            Подготовить слияние
          </button>
        </div>
      </section>

      {proposal && (
        <section className="panel conflict-panel">
          <div className="conflict-panel-head">
            <h2 className="panel-title">Разрешение конфликтов</h2>
            <span className="muted">
              {resolved} из {total} разрешено
            </span>
            <span className="conflict-panel-spacer" />
            <button className="btn btn-ghost" onClick={cancelProposal} disabled={busy}>
              Отменить
            </button>
            <button
              className="btn btn-primary"
              onClick={handleApply}
              disabled={busy || resolved < total}
              title={resolved < total ? "Сначала разрешите все конфликты" : undefined}
            >
              Применить слияние
            </button>
          </div>
          <p className="panel-hint">
            Вариант по умолчанию для нерешённых конфликтов — «наш». Кнопки
            «Всё наше / Всё ихнее» заполняют решения по файлу целиком.
          </p>
          {proposal.files.filter((f) => f.status === "merged").length > 0 && (
            <div className="detail-label">
              Будут применены без конфликтов:{" "}
              {proposal.files
                .filter((f) => f.status === "merged")
                .map((f) => f.path)
                .join(", ")}
            </div>
          )}
          {conflictingFiles.map((file) => (
            <ConflictFile
              key={file.path}
              file={file}
              decision={decisions.get(file.path)}
              onChange={(next) =>
                setDecisions((prev) => {
                  const nextMap = new Map(prev);
                  nextMap.set(file.path, next);
                  return nextMap;
                })
              }
            />
          ))}
        </section>
      )}

      {report && (
        <section className="panel">
          <h2 className="panel-title">Отчёт о слиянии</h2>
          {report.snapshot ? (
            <div className="alert alert-ok">
              Слияние выполнено. Новый снимок:{" "}
              <span className="mono">{shortHash(report.snapshot.hash)}</span>
            </div>
          ) : (
            <div className="alert alert-error">
              Слияние отменено: {report.conflicts.length} конфликт(ов). Репозиторий
              не изменился.
            </div>
          )}

          {report.merged.length > 0 && (
            <>
              <div className="detail-label">Принесённые изменения</div>
              <ul className="merge-list">
                {report.merged.map((file) => (
                  <li key={file} className="merge-ok">
                    {file}
                  </li>
                ))}
              </ul>
            </>
          )}
          {report.conflicts.length > 0 && (
            <>
              <div className="detail-label">Расхождения</div>
              <ul className="merge-list">
                {report.conflicts.map((c) => (
                  <li key={c.file} className="merge-conflict">
                    <b>{c.file}</b> — {c.reason}
                  </li>
                ))}
              </ul>
            </>
          )}
        </section>
      )}

      {error && <div className="alert alert-error">{error}</div>}
    </div>
  );
}

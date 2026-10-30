import { useCallback, useEffect, useState } from "react";
import {
  api,
  shortHash,
  type MergeReport,
  type RepoState,
} from "../api";

interface Props {
  path: string;
  flash: (message: string) => void;
}

export default function MergeScreen({ path, flash }: Props) {
  const [state, setState] = useState<RepoState | null>(null);
  const [source, setSource] = useState("");
  const [newBranch, setNewBranch] = useState("");
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

  const handleMerge = async () => {
    if (!source) return;
    setBusy(true);
    setError(null);
    try {
      const result = await api.mergeBranch(path, source);
      setReport(result);
      if (result.snapshot) {
        flash("Слияние выполнено, снимок создан");
      } else {
        flash("Слияние отменено: есть неразрешимые расхождения");
      }
      await reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
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
                  onClick={() => handleSwitch(b.name)}
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
            placeholder="Имя новой ветви (например, hypothesis-low-ph)"
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
        <h2 className="panel-title">Слияние гипотез</h2>
        <p className="panel-hint">
          Ветка-источник вливается в текущую ветку{" "}
          <span className="badge badge-branch">{state.current_branch}</span>.
          Текстовые файлы и CSV объединяются автоматически; при неразрешимых
          расхождениях слияние отменяется целиком.
        </p>
        <div className="save-bar">
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
            onClick={handleMerge}
            disabled={busy || !source}
          >
            Слить
          </button>
        </div>
      </section>

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
              <div className="detail-label">Расхождения (красным)</div>
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

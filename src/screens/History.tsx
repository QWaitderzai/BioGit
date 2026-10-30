import { useCallback, useEffect, useState } from "react";
import {
  api,
  formatTime,
  shortHash,
  type RepoState,
  type Snapshot,
} from "../api";

interface Props {
  path: string;
  flash: (message: string) => void;
}

export default function History({ path, flash }: Props) {
  const [branches, setBranches] = useState<string[]>([]);
  const [branch, setBranch] = useState("");
  const [snaps, setSnaps] = useState<Snapshot[]>([]);
  const [selected, setSelected] = useState<Snapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const loadBranches = useCallback(async () => {
    try {
      const state: RepoState = await api.openRepo(path);
      setBranches(state.branches.map((b) => b.name));
      setBranch((current) => current || state.current_branch);
    } catch (e) {
      setError(String(e));
    }
  }, [path]);

  const loadHistory = useCallback(async () => {
    if (!branch) return;
    try {
      const list = await api.listHistory(path, branch);
      setSnaps(list);
      setSelected(null);
    } catch (e) {
      setError(String(e));
    }
  }, [path, branch]);

  useEffect(() => {
    void loadBranches();
  }, [loadBranches]);

  useEffect(() => {
    void loadHistory();
  }, [loadHistory]);

  const handleRestore = async () => {
    if (!selected) return;
    setBusy(true);
    setError(null);
    try {
      const count = await api.restoreSnapshot(path, selected.hash);
      flash(`Машина времени: восстановлено файлов — ${count}`);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const paramEntries = selected?.parameters
    ? Object.entries(selected.parameters).slice(0, 8)
    : [];

  return (
    <div className="history">
      <div className="toolbar">
        <label className="label" htmlFor="branch-select">
          Ветка:
        </label>
        <select
          id="branch-select"
          className="input select"
          value={branch}
          onChange={(e) => setBranch(e.target.value)}
        >
          {branches.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
        <span className="muted">{snaps.length} снимков</span>
      </div>

      {snaps.length === 0 ? (
        <p className="empty">На этой ветке пока нет снимков</p>
      ) : (
        <div className="timeline">
          {snaps.map((snap, index) => (
            <button
              key={snap.hash}
              className={`timeline-card ${selected?.hash === snap.hash ? "timeline-card-active" : ""}`}
              onClick={() => setSelected(snap)}
            >
              <div className="timeline-dot" data-latest={index === 0}>
                {index === 0 ? "•" : ""}
              </div>
              <div className="timeline-hash">{shortHash(snap.hash)}</div>
              <div className="timeline-time">{formatTime(snap.timestamp)}</div>
              <div className="timeline-message">{snap.message}</div>
              <div className="timeline-meta">
                {snap.author} · {snap.files_count} файл(ов)
              </div>
              {snap.parent.length > 1 && (
                <div className="badge badge-merge">слияние</div>
              )}
            </button>
          ))}
        </div>
      )}

      {selected && (
        <section className="detail-card">
          <h3 className="panel-title">
            Снимок <span className="mono">{shortHash(selected.hash)}</span>
          </h3>
          <div className="detail-grid">
            <div>
              <div className="detail-label">Автор</div>
              <div>{selected.author}</div>
            </div>
            <div>
              <div className="detail-label">Ветка</div>
              <div>{selected.branch}</div>
            </div>
            <div>
              <div className="detail-label">Время</div>
              <div>{formatTime(selected.timestamp)}</div>
            </div>
            <div>
              <div className="detail-label">Файлов</div>
              <div>{selected.files_count}</div>
            </div>
          </div>
          <div className="detail-label">Параметры эксперимента («рецепт»)</div>
          {paramEntries.length === 0 ? (
            <p className="empty">параметры не заданы</p>
          ) : (
            <div className="chips">
              {paramEntries.map(([key, value]) => (
                <span key={key} className="chip">
                  {key}: {String(value)}
                </span>
              ))}
            </div>
          )}
          <div className="detail-actions">
            <button className="btn btn-primary" onClick={handleRestore} disabled={busy}>
              Машина времени: вернуть файлы из этого снимка
            </button>
          </div>
        </section>
      )}

      {error && <div className="alert alert-error">{error}</div>}
    </div>
  );
}

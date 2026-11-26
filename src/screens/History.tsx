import { useCallback, useEffect, useState } from "react";
import {
  api,
  formatTime,
  shortHash,
  type FileDiff,
  type GraphData,
  type RepoState,
  type Snapshot,
} from "../api";
import GitGraph from "../components/GitGraph";
import DiffView from "../components/DiffView";
import { Modal, useConfirm } from "../components/Modal";

interface Props {
  path: string;
  flash: (message: string) => void;
}

export default function History({ path, flash }: Props) {
  const confirm = useConfirm();
  const [branches, setBranches] = useState<string[]>([]);
  const [branch, setBranch] = useState("");
  const [snaps, setSnaps] = useState<Snapshot[]>([]);
  const [graph, setGraph] = useState<GraphData | null>(null);
  const [selected, setSelected] = useState<Snapshot | null>(null);
  const [diffModal, setDiffModal] = useState<{ diffs: FileDiff[]; title: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [viewMode, setViewMode] = useState<"graph" | "timeline">("graph");

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

  const loadGraph = useCallback(async () => {
    try {
      const data = await api.historyGraph(path);
      setGraph(data);
    } catch (e) {
      setError(String(e));
    }
  }, [path]);

  useEffect(() => {
    void loadBranches();
    void loadGraph();
  }, [loadBranches, loadGraph]);

  useEffect(() => {
    void loadHistory();
  }, [loadHistory]);

  const handleRestore = async () => {
    if (!selected) return;
    const ok = await confirm({
      title: "Вернуть состояние?",
      message: (
        <>
          Файлы рабочей папки будут перезаписаны версиями из снимка{" "}
          <code>{shortHash(selected.hash)}</code>. История останется без изменений.
        </>
      ),
      confirmLabel: "Вернуть",
    });
    if (!ok) return;
    setBusy(true);
    setError(null);
    try {
      const count = await api.restoreSnapshot(path, selected.hash);
      flash(`Восстановлено файлов — ${count}`);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // diff выбранного снимка против предыдущего в той же линии
  const showDiffToParent = async () => {
    if (!selected) return;
    const parent = selected.parent[0];
    if (!parent) {
      setDiffModal({
        diffs: [],
        title: `Снимок ${shortHash(selected.hash)} — начальный, родителя нет`,
      });
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const diffs = await api.diffBetween(path, parent, selected.hash);
      setDiffModal({
        diffs,
        title: `Изменения: ${shortHash(parent)} → ${shortHash(selected.hash)}`,
      });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // diff произвольной пары: выбранная карточка — «новый», следующий клик — «старый»
  const [compareBase, setCompareBase] = useState<Snapshot | null>(null);
  const handleCompareClick = async (snap: Snapshot) => {
    if (!compareBase) {
      setCompareBase(snap);
      return;
    }
    const base = compareBase;
    setCompareBase(null);
    if (base.hash === snap.hash) return;
    setBusy(true);
    setError(null);
    try {
      const diffs = await api.diffBetween(path, base.hash, snap.hash);
      setDiffModal({
        diffs,
        title: `Изменения: ${shortHash(base.hash)} → ${shortHash(snap.hash)}`,
      });
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
        <span className="muted">
          {viewMode === "graph"
            ? `${graph?.nodes.length ?? 0} снимков (все ветви)`
            : `${snaps.length} снимков`}
        </span>
        <span className="toolbar-spacer" />
        <button
          type="button"
          className={`btn btn-sm ${viewMode === "graph" ? "btn-primary" : "btn-ghost"}`}
          onClick={() => setViewMode("graph")}
        >
          Граф ветвлений
        </button>
        <button
          type="button"
          className={`btn btn-sm ${viewMode === "timeline" ? "btn-primary" : "btn-ghost"}`}
          onClick={() => setViewMode("timeline")}
        >
          Линейный список
        </button>
      </div>

      {viewMode === "graph" && graph ? (
        <GitGraph
          graph={graph}
          selectedHash={selected?.hash ?? null}
          onSelect={(hash) => {
            const snap = [...snaps, ...(graph?.nodes.map(toCardSnapshot) ?? [])].find(
              (s) => s.hash === hash,
            );
            if (snap) setSelected(snap);
          }}
        />
      ) : snaps.length === 0 ? (
        <p className="empty">На этой ветке нет снимков</p>
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

      {compareBase && (
        <div className="alert alert-compare">
          База для сравнения: <code>{shortHash(compareBase.hash)}</code> — кликните
          другой снимок, чтобы увидеть отличия, или{" "}
          <button type="button" className="btn btn-ghost btn-sm" onClick={() => setCompareBase(null)}>
            отменить
          </button>
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
            <div>
              <div className="detail-label">Родители</div>
              <div className="mono small">
                {selected.parent.length > 0
                  ? selected.parent.map((p) => shortHash(p)).join(", ")
                  : "—"}
              </div>
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
            <button className="btn btn-ghost" onClick={() => void showDiffToParent()} disabled={busy}>
              Изменения относительно предыдущего
            </button>
            <button
              className="btn btn-ghost"
              onClick={() => void handleCompareClick(selected)}
              disabled={busy}
              title="Затем кликните другой снимок для сравнения"
            >
              Сравнить с…
            </button>
            <button className="btn btn-primary" onClick={() => void handleRestore()} disabled={busy}>
              Вернуть файлы из этого снимка
            </button>
          </div>
        </section>
      )}

      {diffModal && (
        <Modal title={diffModal.title} onClose={() => setDiffModal(null)} wide>
          <DiffView diffs={diffModal.diffs} />
        </Modal>
      )}

      {error && <div className="alert alert-error">{error}</div>}
    </div>
  );
}

// узел графа не содержит параметров/дерева — собираем карточку из него
function toCardSnapshot(node: GraphData["nodes"][number]): Snapshot {
  return {
    hash: node.hash,
    timestamp: node.timestamp,
    author: node.author,
    message: node.message,
    parent: node.parents,
    branch: node.branch,
    tree: "",
    parameters: null,
    files_count: node.files_count,
  };
}

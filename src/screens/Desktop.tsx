import { useCallback, useEffect, useState } from "react";
import { api, formatSize, shortHash, type RepoState, type Snapshot } from "../api";

interface Props {
  path: string;
  flash: (message: string) => void;
}

const DEFAULT_PARAMS = `{
  "temperature": 310,
  "ph": 7.4,
  "iterations": 500
}`;

export default function Desktop({ path, flash }: Props) {
  const [state, setState] = useState<RepoState | null>(null);
  const [message, setMessage] = useState("");
  const [author, setAuthor] = useState(() => localStorage.getItem("biogit.author") ?? "");
  const [paramsText, setParamsText] = useState(DEFAULT_PARAMS);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const opened = await api.openRepo(path);
      setState(opened);
      if (opened.latest?.parameters && typeof opened.latest.parameters === "object") {
        setParamsText(JSON.stringify(opened.latest.parameters, null, 2));
      }
    } catch (e) {
      setError(String(e));
    }
  }, [path]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const handleSave = async () => {
    // проверяем JSON до отправки в ядро
    try {
      JSON.parse(paramsText);
    } catch (e) {
      setError(`Параметры — некорректный JSON: ${String(e)}`);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const snap = await api.saveSnapshot(path, message, author, paramsText);
      localStorage.setItem("biogit.author", author);
      flash(`Снимок ${shortHash(snap.hash)} сохранён: файлов — ${snap.files_count}`);
      setMessage("");
      await reload();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleRestore = async (snap: Snapshot) => {
    setBusy(true);
    setError(null);
    try {
      const count = await api.restoreSnapshot(path, snap.hash);
      flash(`Машина времени: восстановлено файлов — ${count}`);
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
    <div className="desktop">
      <div className="panel-grid">
        <section className="panel">
          <h2 className="panel-title">Сырьё</h2>
          <p className="panel-hint">Файлы рабочей папки — то, что попадёт в снимок</p>
          {state.files.length === 0 ? (
            <p className="empty">Папка пуста — скопируйте сюда файлы данных</p>
          ) : (
            <ul className="file-list">
              {state.files.map((f) => (
                <li key={f.name} className="file-row">
                  <span className="file-name">{f.name}</span>
                  <span className="file-size">{formatSize(f.size)}</span>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="panel">
          <h2 className="panel-title">Рецепт</h2>
          <p className="panel-hint">Параметры эксперимента (JSON) — сохраняются вместе со снимком</p>
          <textarea
            className="textarea"
            spellCheck={false}
            value={paramsText}
            onChange={(e) => setParamsText(e.target.value)}
            rows={12}
          />
        </section>

        <section className="panel">
          <h2 className="panel-title">Результат</h2>
          <p className="panel-hint">Последний снимок текущей ветки</p>
          {state.latest ? (
            <div className="snapshot-card">
              <div className="snapshot-hash">{shortHash(state.latest.hash)}</div>
              <div className="snapshot-message">{state.latest.message}</div>
              <div className="snapshot-meta">
                {state.latest.author} · {state.latest.files_count} файл(ов)
              </div>
              <button
                className="btn btn-ghost btn-sm"
                onClick={() => handleRestore(state.latest!)}
                disabled={busy}
              >
                Вернуть это состояние
              </button>
            </div>
          ) : (
            <p className="empty">Снимков ещё нет — сделайте первый</p>
          )}
        </section>
      </div>

      <section className="save-bar">
        <input
          className="input"
          placeholder="Автор (например, Дмитрий)"
          value={author}
          onChange={(e) => setAuthor(e.target.value)}
        />
        <input
          className="input input-grow"
          placeholder="Что изменилось в эксперименте?"
          value={message}
          onChange={(e) => setMessage(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleSave()}
        />
        <button className="btn btn-primary" onClick={handleSave} disabled={busy}>
          Сохранить снимок
        </button>
      </section>

      {error && <div className="alert alert-error">{error}</div>}
    </div>
  );
}

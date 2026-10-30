import { useCallback, useState } from "react";
import { api, type RepoState } from "./api";
import Desktop from "./screens/Desktop";
import History from "./screens/History";
import MergeScreen from "./screens/Merge";

type Tab = "desktop" | "history" | "merge";

const TABS: { id: Tab; label: string }[] = [
  { id: "desktop", label: "Рабочий стол" },
  { id: "history", label: "История" },
  { id: "merge", label: "Ветви и слияние" },
];

export default function App() {
  const [path, setPath] = useState(() => localStorage.getItem("biogit.path") ?? "");
  const [state, setState] = useState<RepoState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("desktop");
  const [busy, setBusy] = useState(false);

  const flash = useCallback((message: string) => {
    setNotice(message);
    window.setTimeout(() => setNotice(null), 4500);
  }, []);

  const rememberPath = (value: string) => {
    setPath(value);
    localStorage.setItem("biogit.path", value);
  };

  const handleOpen = async () => {
    setBusy(true);
    setError(null);
    try {
      const opened = await api.openRepo(path);
      setState(opened);
      rememberPath(path);
    } catch (e) {
      setError(`Не удалось открыть репозиторий: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleCreate = async () => {
    setBusy(true);
    setError(null);
    try {
      const created = await api.initRepo(path);
      setState(created);
      rememberPath(path);
      flash("Репозиторий BioGit создан");
    } catch (e) {
      setError(`Не удалось создать репозиторий: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  if (!state) {
    return (
      <div className="welcome">
        <div className="welcome-card">
          <div className="welcome-logo">
            <img src="/biogit-icon.svg" alt="" aria-hidden="true" />
            <h1>BioGit</h1>
          </div>
          <p className="welcome-subtitle">
            Система версионирования экспериментальных данных.
            <br />
            Укажите папку с данными эксперимента — BioGit будет хранить её историю в скрытой
            папке <code>.biogit</code>.
          </p>
          <input
            className="input path-input"
            type="text"
            placeholder="Например: D:\\Эксперименты\\pcr-2026-10"
            value={path}
            onChange={(e) => setPath(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleOpen()}
          />
          <div className="welcome-actions">
            <button className="btn btn-primary" onClick={handleOpen} disabled={busy || !path.trim()}>
              Открыть репозиторий
            </button>
            <button className="btn btn-ghost" onClick={handleCreate} disabled={busy || !path.trim()}>
              Создать новый
            </button>
          </div>
          {error && <div className="alert alert-error">{error}</div>}
          <p className="hint">
            Нет папки с данными? Скопируйте папку <code>demo_experiment</code> из состава проекта
            и укажите её путь.
          </p>
        </div>
        {notice && <div className="toast">{notice}</div>}
      </div>
    );
  }

  return (
    <div className="app">
      <header className="topbar">
        <div className="topbar-brand">BioGit</div>
        <div className="topbar-path" title={state.path}>
          {state.path}
        </div>
        <div className="badge badge-branch">{state.current_branch}</div>
        <button
          className="btn btn-ghost btn-sm"
          onClick={() => {
            setState(null);
            setError(null);
          }}
        >
          Сменить папку
        </button>
      </header>

      <nav className="tabs">
        {TABS.map((t) => (
          <button
            key={t.id}
            className={`tab ${tab === t.id ? "tab-active" : ""}`}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </nav>

      <main className="content">
        {tab === "desktop" && <Desktop path={state.path} flash={flash} />}
        {tab === "history" && <History path={state.path} flash={flash} />}
        {tab === "merge" && <MergeScreen path={state.path} flash={flash} />}
      </main>

      {notice && <div className="toast">{notice}</div>}
    </div>
  );
}

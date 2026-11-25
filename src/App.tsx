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

const RECENT_KEY = "biogit.recent";

function loadRecent(): string[] {
  try {
    const raw = JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
    return Array.isArray(raw) ? raw.filter((p): p is string => typeof p === "string") : [];
  } catch {
    return [];
  }
}

export default function App() {
  const [path, setPath] = useState(() => localStorage.getItem("biogit.path") ?? "");
  const [state, setState] = useState<RepoState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("desktop");
  const [busy, setBusy] = useState(false);
  const [recent, setRecent] = useState<string[]>(loadRecent);

  const flash = useCallback((message: string) => {
    setNotice(message);
    window.setTimeout(() => setNotice(null), 4500);
  }, []);

  const rememberPath = (value: string) => {
    setPath(value);
    localStorage.setItem("biogit.path", value);
  };

  const pushRecent = (value: string) => {
    const next = [value, ...recent.filter((p) => p !== value)].slice(0, 6);
    localStorage.setItem(RECENT_KEY, JSON.stringify(next));
    setRecent(next);
  };

  /** Открывает папку как репозиторий; при createIfMissing — инициализирует, если .biogit нет. */
  const openFolderPath = async (folder: string, createIfMissing: boolean): Promise<boolean> => {
    setBusy(true);
    setError(null);
    try {
      try {
        const opened = await api.openRepo(folder);
        setState(opened);
        rememberPath(folder);
        pushRecent(folder);
        return true;
      } catch (openError) {
        if (!createIfMissing) {
          setError(
            `Папка «${folder}» не является репозиторием BioGit (нет папки .biogit). Нажмите «Создать новый», чтобы инициализировать её.`,
          );
          return false;
        }
        const created = await api.initRepo(folder);
        setState(created);
        rememberPath(folder);
        pushRecent(folder);
        flash("Репозиторий BioGit создан");
        return true;
      }
    } catch (e) {
      setError(`Не удалось открыть папку «${folder}»: ${String(e)}`);
      return false;
    } finally {
      setBusy(false);
    }
  };

  /**
   * Системный диалог «Выбрать папку» (как «Open Folder» в VSCode).
   * Если в выбранной папке нет репозитория — предлагает создать его.
   */
  const pickAndOpen = async () => {
    setError(null);
    let folder: string | null = null;
    try {
      folder = await api.pickFolder();
    } catch (e) {
      setError(`Не удалось открыть диалог выбора папки: ${String(e)}`);
      return;
    }
    if (!folder) return; // пользователь отменил выбор
    rememberPath(folder);
    try {
      await api.openRepo(folder);
      await openFolderPath(folder, false);
    } catch {
      const create = window.confirm(
        `В папке нет репозитория BioGit:\n${folder}\n\nСоздать новый репозиторий в этой папке?`,
      );
      if (create) {
        await openFolderPath(folder, true);
      }
    }
  };

  const handleOpen = () => openFolderPath(path, false);
  const handleCreate = () => openFolderPath(path, true);

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
            Откройте любую папку с данными эксперимента — BioGit будет хранить её историю в скрытой
            папке <code>.biogit</code>.
          </p>
          <button
            type="button"
            className="btn btn-primary btn-open-folder"
            onClick={() => void pickAndOpen()}
            disabled={busy}
          >
            Выбрать папку…
          </button>
          <div className="welcome-manual">
            <input
              className="input path-input"
              type="text"
              placeholder="или введите путь вручную: D:\\Эксперименты\\pcr-2026-10"
              value={path}
              onChange={(e) => setPath(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleOpen()}
            />
            <div className="welcome-actions">
              <button className="btn btn-ghost" onClick={handleOpen} disabled={busy || !path.trim()}>
                Открыть репозиторий
              </button>
              <button className="btn btn-ghost" onClick={handleCreate} disabled={busy || !path.trim()}>
                Создать новый
              </button>
            </div>
          </div>
          {recent.length > 0 && (
            <div className="recent">
              <div className="detail-label">Недавние папки</div>
              {recent.map((folder) => (
                <button
                  type="button"
                  key={folder}
                  className="recent-item"
                  title={folder}
                  onClick={() => void openFolderPath(folder, false)}
                >
                  <span className="mono">{folder}</span>
                </button>
              ))}
            </div>
          )}
          {error && <div className="alert alert-error">{error}</div>}
          <p className="hint">
            Нет папки с данными? Скопируйте папку <code>demo_experiment</code> из состава проекта
            и откройте её через «Выбрать папку…».
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
        <button className="btn btn-ghost btn-sm" onClick={() => void pickAndOpen()} disabled={busy}>
          Открыть папку…
        </button>
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
        {tab === "desktop" && (
          <Desktop path={state.path} flash={flash} openFolderPicker={pickAndOpen} />
        )}
        {tab === "history" && <History path={state.path} flash={flash} />}
        {tab === "merge" && <MergeScreen path={state.path} flash={flash} />}
      </main>

      {notice && <div className="toast">{notice}</div>}
    </div>
  );
}

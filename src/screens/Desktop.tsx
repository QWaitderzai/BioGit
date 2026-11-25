import { useCallback, useEffect, useRef, useState } from "react";
import { api, formatSize, shortHash, type RepoState, type Snapshot } from "../api";
import FileExplorer from "../components/FileExplorer";
import FileEditor, { type OpenTab } from "../components/FileEditor";

interface Props {
  path: string;
  flash: (message: string) => void;
  /** Открывает системный диалог выбора папки и переключается на неё. */
  openFolderPicker: () => Promise<void>;
}

const DEFAULT_PARAMS = `{
  "temperature": 0,
  "ph": 0,
  "iterations": 0
}`;

/** Имя корневой папки для заголовка проводника. */
function rootName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

export default function Desktop({ path, flash, openFolderPicker }: Props) {
  const [state, setState] = useState<RepoState | null>(null);
  const [message, setMessage] = useState("");
  const [author, setAuthor] = useState(() => localStorage.getItem("biogit.author") ?? "");
  const [paramsText, setParamsText] = useState(DEFAULT_PARAMS);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // вкладки встроенного редактора
  const [tabs, setTabs] = useState<OpenTab[]>([]);
  const [activeTab, setActiveTab] = useState<string | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(true);

  // рецепт из последнего снимка подтягивается только при первом открытии папки,
  // чтобы «Обновить» не затирал правки пользователя
  const paramsInitialized = useRef(false);

  const reload = useCallback(async () => {
    try {
      const opened = await api.openRepo(path);
      setState(opened);
      if (!paramsInitialized.current && opened.latest?.parameters) {
        setParamsText(JSON.stringify(opened.latest.parameters, null, 2));
        paramsInitialized.current = true;
      }
    } catch (e) {
      setError(String(e));
    }
  }, [path]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // при смене рабочей папки закрываем вкладки предыдущей
  useEffect(() => {
    setTabs([]);
    setActiveTab(null);
    paramsInitialized.current = false;
  }, [path]);

  const openFile = useCallback(
    async (name: string) => {
      if (tabs.some((t) => t.name === name)) {
        setActiveTab(name);
        return;
      }
      setBusy(true);
      setError(null);
      try {
        const content = await api.readFile(path, name);
        const tab: OpenTab = {
          name,
          content: content.text ?? "",
          saved: content.text ?? "",
          binary: content.is_binary,
          size: content.size,
        };
        setTabs((prev) => [...prev, tab]);
        setActiveTab(name);
      } catch (e) {
        setError(`Не удалось открыть файл «${name}»: ${String(e)}`);
      } finally {
        setBusy(false);
      }
    },
    [path, tabs],
  );

  const closeTab = (name: string) => {
    const index = tabs.findIndex((t) => t.name === name);
    if (index < 0) return;
    const tab = tabs[index];
    if (tab.content !== tab.saved) {
      const reallyClose = window.confirm(
        `В файле «${name}» есть несохранённые изменения.\nЗакрыть вкладку и потерять правки?`,
      );
      if (!reallyClose) return;
    }
    const next = tabs.filter((t) => t.name !== name);
    setTabs(next);
    if (activeTab === name) {
      setActiveTab(next.length ? next[Math.min(index, next.length - 1)].name : null);
    }
  };

  const changeTab = (name: string, content: string) => {
    setTabs((prev) => prev.map((t) => (t.name === name ? { ...t, content } : t)));
  };

  const saveFile = async (name: string) => {
    const tab = tabs.find((t) => t.name === name);
    if (!tab || tab.binary) return;
    setBusy(true);
    setError(null);
    try {
      const size = await api.writeFile(path, name, tab.content);
      setTabs((prev) =>
        prev.map((t) => (t.name === name ? { ...t, saved: t.content, size } : t)),
      );
      flash(`Файл ${name} сохранён (${formatSize(size)})`);
      await reload();
    } catch (e) {
      setError(`Не удалось сохранить файл «${name}»: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

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
      flash(`Восстановлено файлов — ${count}`);
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
    <div className="ide">
      <aside className={`explorer ${sidebarOpen ? "" : "explorer-hidden"}`}>
        <div className="explorer-head">
          <span>Проводник</span>
          <div className="explorer-actions">
            <button
              type="button"
              className="btn btn-ghost btn-sm"
              onClick={() => void openFolderPicker()}
              title="Открыть любую папку через системный диалог"
            >
              Открыть папку…
            </button>
          </div>
        </div>
        <button
          type="button"
          className="explorer-root"
          title={path}
          onClick={() => void openFolderPicker()}
        >
          {rootName(path)}
        </button>
        <FileExplorer files={state.files} activeFile={activeTab} onOpenFile={(name) => void openFile(name)} />
        <div className="explorer-foot">
          <button type="button" className="btn btn-ghost btn-sm" onClick={() => void reload()}>
            Обновить
          </button>
          <button
            type="button"
            className="btn btn-ghost btn-sm"
            onClick={() => setSidebarOpen(false)}
            title="Скрыть проводник"
          >
            Скрыть
          </button>
          <span className="muted small explorer-count">{state.files.length} файл(ов)</span>
        </div>
      </aside>

      <div className="ide-main">
        <FileEditor
          tabs={tabs}
          active={activeTab}
          busy={busy}
          sidebarOpen={sidebarOpen}
          onSelect={setActiveTab}
          onClose={closeTab}
          onChange={changeTab}
          onSave={(name) => void saveFile(name)}
          onToggleSidebar={() => setSidebarOpen(true)}
        />

        <div className="ide-bottom">
          <section className="panel">
            <h2 className="panel-title">Рецепт</h2>
            <p className="panel-hint">Параметры эксперимента (JSON) — сохраняются вместе со снимком</p>
            <textarea
              className="textarea"
              spellCheck={false}
              value={paramsText}
              onChange={(e) => setParamsText(e.target.value)}
              rows={6}
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
                  type="button"
                  className="btn btn-ghost btn-sm"
                  onClick={() => void handleRestore(state.latest!)}
                  disabled={busy}
                >
                  Вернуть это состояние
                </button>
              </div>
            ) : (
              <p className="empty">Снимков нет — сохраните первый снимок</p>
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
            onKeyDown={(e) => e.key === "Enter" && void handleSave()}
          />
          <button type="button" className="btn btn-primary" onClick={() => void handleSave()} disabled={busy}>
            Сохранить снимок
          </button>
        </section>

        {error && <div className="alert alert-error">{error}</div>}
      </div>
    </div>
  );
}

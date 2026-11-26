import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  formatSize,
  shortHash,
  type FileDiff,
  type FileStatus,
  type RepoState,
  type Snapshot,
} from "../api";
import FileExplorer from "../components/FileExplorer";
import FileEditor, { type OpenTab } from "../components/FileEditor";
import DiffView from "../components/DiffView";
import { Modal, useConfirm } from "../components/Modal";

interface Props {
  path: string;
  flash: (message: string) => void;
  openFolderPicker: () => Promise<void>;
}

const DEFAULT_PARAMS = `{
  "temperature": 0,
  "ph": 0,
  "iterations": 0
}`;

// последнее имя в пути — его и показываем в шапке проводника
function rootName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

export default function Desktop({ path, flash, openFolderPicker }: Props) {
  const confirm = useConfirm();
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
  const [sheetDirty, setSheetDirty] = useState(false);

  // статус рабочих файлов (индикаторы М/Н/У)
  const [statuses, setStatuses] = useState<FileStatus[] | null>(null);
  const [diffModal, setDiffModal] = useState<{ diffs: FileDiff[]; title: string } | null>(null);

  // рецепт из последнего снимка подтягиваем только при открытии папки,
  // иначе «Обновить» будет затирать правки пользователя
  const paramsInitialized = useRef(false);

  const reload = useCallback(async () => {
    try {
      const opened = await api.openRepo(path);
      setState(opened);
      if (!paramsInitialized.current && opened.latest?.parameters) {
        setParamsText(JSON.stringify(opened.latest.parameters, null, 2));
        paramsInitialized.current = true;
      }
      const status = await api.fileStatus(path);
      setStatuses(status);
    } catch (e) {
      setError(String(e));
    }
  }, [path]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // сменилась рабочая папка — закрываем вкладки старой
  useEffect(() => {
    setTabs([]);
    setActiveTab(null);
    paramsInitialized.current = false;
    setSheetDirty(false);
  }, [path]);

  const statusMap = new Map((statuses ?? []).map((s) => [s.name, s.state]));
  const deletedFiles = (statuses ?? []).filter((s) => s.state === "deleted").map((s) => s.name);
  const counts = {
    modified: (statuses ?? []).filter((s) => s.state === "modified").length,
    added: (statuses ?? []).filter((s) => s.state === "added").length,
    deleted: deletedFiles.length,
  };

  const openFile = useCallback(
    async (name: string) => {
      if (tabs.some((t) => t.name === name)) {
        setActiveTab(name);
        return;
      }
      if (/\.(xlsx|xls|xlsm|xlsb|ods)$/i.test(name)) {
        // книга Excel — открывается табличным редактором без загрузки текста
        setTabs((prev) => [
          ...prev,
          { name, content: "", saved: "", binary: false, size: 0 },
        ]);
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

  const closeTab = async (name: string) => {
    const index = tabs.findIndex((t) => t.name === name);
    if (index < 0) return;
    const tab = tabs[index];
    const isSheet = /\.(xlsx|xls|xlsm|xlsb|ods)$/i.test(name);
    if ((tab.content !== tab.saved && !isSheet) || (isSheet && sheetDirty && activeTab === name)) {
      const reallyClose = await confirm({
        title: "Закрыть вкладку?",
        message: `В файле «${name}» есть несохранённые изменения. Закрыть вкладку и потерять правки?`,
        confirmLabel: "Закрыть и потерять",
        danger: true,
      });
      if (!reallyClose) return;
    }
    if (isSheet) {
      void api.sheetClose().catch(() => {});
      setSheetDirty(false);
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

  const saveSheet = async () => {
    setBusy(true);
    setError(null);
    try {
      const savedName = await api.sheetSave(path);
      flash(`Таблица ${savedName} сохранена`);
      setSheetDirty(false);
      await reload();
    } catch (e) {
      setError(`Не удалось сохранить таблицу: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const handleSave = async () => {
    // json проверяем на клиенте, чтобы не гонять заведомо битый в ядро
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
    const ok = await confirm({
      title: "Вернуть состояние?",
      message: (
        <>
          Файлы рабочей папки будут перезаписаны версиями из снимка{" "}
          <code>{shortHash(snap.hash)}</code>. История останется без изменений.
        </>
      ),
      confirmLabel: "Вернуть",
    });
    if (!ok) return;
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

  const showWorkingDiff = async () => {
    setBusy(true);
    setError(null);
    try {
      const diffs = await api.diffWorking(path);
      setDiffModal({ diffs, title: "Изменения к следующему снимку" });
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!state) {
    return <div className="alert alert-error">{error ?? "Загрузка…"}</div>;
  }

  const hasPendingChanges = counts.modified + counts.added + counts.deleted > 0;

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
        <FileExplorer
          files={state.files}
          activeFile={activeTab}
          statuses={statuses ? statusMap : null}
          deletedFiles={deletedFiles}
          onOpenFile={(name) => void openFile(name)}
        />
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
          repoPath={path}
          tabs={tabs}
          active={activeTab}
          busy={busy}
          sidebarOpen={sidebarOpen}
          sheetDirty={sheetDirty}
          onSelect={setActiveTab}
          onClose={(name) => void closeTab(name)}
          onChange={changeTab}
          onSave={(name) => void saveFile(name)}
          onSheetSave={() => void saveSheet()}
          onSheetDirtyChange={setSheetDirty}
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
          <div className="status-chips" title="Состояние рабочей папки относительно последнего снимка">
            {hasPendingChanges ? (
              <>
                {counts.modified > 0 && (
                  <button type="button" className="chip chip-modified" onClick={() => void showWorkingDiff()}>
                    М {counts.modified}
                  </button>
                )}
                {counts.added > 0 && (
                  <button type="button" className="chip chip-added" onClick={() => void showWorkingDiff()}>
                    Н {counts.added}
                  </button>
                )}
                {counts.deleted > 0 && (
                  <button type="button" className="chip chip-deleted" onClick={() => void showWorkingDiff()}>
                    У {counts.deleted}
                  </button>
                )}
                <button
                  type="button"
                  className="btn btn-ghost btn-sm"
                  onClick={() => void showWorkingDiff()}
                >
                  Показать изменения
                </button>
              </>
            ) : (
              <span className="chip chip-clean">рабочая папка чистая</span>
            )}
          </div>
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

      {diffModal && (
        <Modal title={diffModal.title} onClose={() => setDiffModal(null)} wide>
          <DiffView diffs={diffModal.diffs} />
        </Modal>
      )}
    </div>
  );
}

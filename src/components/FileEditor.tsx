import { useState } from "react";
import type { KeyboardEvent } from "react";
import { formatSize, isCsvFile, isSheetFile } from "../api";
import CsvTable from "./CsvTable";
import SheetEditor from "./SheetEditor";

// редактор с вкладками как в vscode: точка на вкладке — есть несохранённые правки,
// сохранение по Ctrl+S или кнопкой, двоичные файлы можно только посмотреть.
// CSV открываются красивой таблицей, таблицы (xlsx) — табличным редактором.

export interface OpenTab {
  name: string;
  content: string; // что сейчас в textarea
  saved: string; // что последний раз записали на диск
  binary: boolean;
  size: number;
}

interface Props {
  repoPath: string;
  tabs: OpenTab[];
  active: string | null;
  busy: boolean;
  sidebarOpen: boolean;
  sheetDirty: boolean;
  onSelect: (name: string) => void;
  onClose: (name: string) => void;
  onChange: (name: string, content: string) => void;
  onSave: (name: string) => void;
  onSheetSave: () => void;
  onSheetDirtyChange: (dirty: boolean) => void;
  onToggleSidebar: () => void;
}

function FileName({ name }: { name: string }) {
  const parts = name.split("/");
  return (
    <>
      <span className="tabfile-dir">{parts.length > 1 ? `${parts.slice(0, -1).join("/")}/` : ""}</span>
      <span>{parts[parts.length - 1]}</span>
    </>
  );
}

export default function FileEditor({
  repoPath,
  tabs,
  active,
  busy,
  sidebarOpen,
  sheetDirty,
  onSelect,
  onClose,
  onChange,
  onSave,
  onSheetSave,
  onSheetDirtyChange,
  onToggleSidebar,
}: Props) {
  const [viewMode, setViewMode] = useState<"table" | "text">("table");
  const current = tabs.find((t) => t.name === active) ?? null;
  const isDirty = current ? current.content !== current.saved : false;

  const handleKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      if (current && !current.binary && isDirty) {
        onSave(current.name);
      }
    }
  };

  return (
    <div className="editor-area">
      <div className="tabsbar">
        {!sidebarOpen && (
          <button
            type="button"
            className="btn btn-ghost btn-sm tabsbar-sidebar"
            onClick={onToggleSidebar}
            title="Показать проводник файлов"
          >
            Показать проводник
          </button>
        )}
        {tabs.length === 0 && <div className="tabsbar-empty" />}
        {tabs.map((tab) => {
          const dirty = tab.content !== tab.saved || (isSheetFile(tab.name) && sheetDirty && active === tab.name);
          return (
            <div
              key={tab.name}
              className={`tabfile ${active === tab.name ? "tabfile-active" : ""}`}
              onClick={() => onSelect(tab.name)}
              title={tab.name}
            >
              <FileName name={tab.name} />
              {dirty && (
                <span className="tab-dirty" title="Есть несохранённые изменения">
                  ●
                </span>
              )}
              <button
                type="button"
                className="tab-close"
                title="Закрыть вкладку"
                onClick={(e) => {
                  e.stopPropagation();
                  onClose(tab.name);
                }}
              >
                ×
              </button>
            </div>
          );
        })}
      </div>

      {current === null ? (
        <div className="start-page">
          <div className="start-page-card">
            <h2>Файлы эксперимента</h2>
            <p className="muted">
              Выберите файл в проводнике слева — он откроется прямо в BioGit.
              <br />
              CSV — в виде таблицы, книги Excel (xlsx) — в табличном редакторе
              с формулами, текст — как обычно.
            </p>
          </div>
        </div>
      ) : null}

      {/* Табличные редакторы держатся смонтированными всегда,
          чтобы не терять сессию Excel и несохранённые правки при
          переключении вкладок */}
      {tabs.filter((t) => isSheetFile(t.name)).map((tab) => (
        <div
          key={`sheet-slot-${tab.name}`}
          hidden={active !== tab.name}
          className="sheet-editor-slot"
        >
          <SheetEditor
            path={repoPath}
            name={tab.name}
            dirty={sheetDirty}
            busy={busy}
            onSave={onSheetSave}
            onDirtyChange={onSheetDirtyChange}
          />
        </div>
      ))}

      {current && current.binary ? (
        <div className="editor">
          <div className="editor-head">
            <span className="mono">{current.name}</span>
            <span className="badge">двоичный файл</span>
          </div>
          <div className="binary-note">
            <p>
              Файл не является текстом (UTF-8), поэтому редактирование в BioGit
              недоступно — это защита от повреждения данных.
            </p>
            <p className="muted">
              Размер: {formatSize(current.size)}. Файл попадёт в снимки как обычные данные.
            </p>
          </div>
        </div>
      ) : current && isCsvFile(current.name) ? (
        <div className="editor">
          <div className="editor-head">
            <span className="mono">{current.name}</span>
            {isDirty && <span className="badge badge-unsaved">не сохранено</span>}
            <span className="editor-head-spacer" />
            <div className="view-switch">
              <button
                type="button"
                className={`btn btn-sm ${viewMode === "table" ? "btn-primary" : "btn-ghost"}`}
                onClick={() => setViewMode("table")}
              >
                Таблица
              </button>
              <button
                type="button"
                className={`btn btn-sm ${viewMode === "text" ? "btn-primary" : "btn-ghost"}`}
                onClick={() => setViewMode("text")}
              >
                Текст
              </button>
            </div>
            <button
              type="button"
              className="btn btn-primary btn-sm"
              onClick={() => onSave(current.name)}
              disabled={busy || !isDirty}
              title="Сохранить файл на диск (Ctrl+S)"
            >
              Сохранить файл
            </button>
          </div>
          {viewMode === "table" ? (
            <div className="csv-editor-area">
              <CsvTable
                text={current.content}
                onChange={(text) => onChange(current.name, text)}
              />
            </div>
          ) : (
            <textarea
              className="editor-textarea"
              spellCheck={false}
              value={current.content}
              onChange={(e) => onChange(current.name, e.target.value)}
              onKeyDown={handleKeyDown}
            />
          )}
          <div className="editor-status">
            <span>UTF-8</span>
            <span>{current.content.split("\n").length} строк</span>
            <span>{current.content.length} символов</span>
            <span className="editor-status-spacer" />
            <span className="muted">
              {isDirty ? "есть изменения — Ctrl+S, чтобы сохранить" : "сохранено"}
            </span>
          </div>
        </div>
      ) : current ? (
        <div className="editor">
          <div className="editor-head">
            <span className="mono">{current.name}</span>
            {isDirty && <span className="badge badge-unsaved">не сохранено</span>}
            <span className="editor-head-spacer" />
            <button
              type="button"
              className="btn btn-primary btn-sm"
              onClick={() => onSave(current.name)}
              disabled={busy || !isDirty}
              title="Сохранить файл на диск (Ctrl+S)"
            >
              Сохранить файл
            </button>
          </div>
          <textarea
            className="editor-textarea"
            spellCheck={false}
            value={current.content}
            onChange={(e) => onChange(current.name, e.target.value)}
            onKeyDown={handleKeyDown}
          />
          <div className="editor-status">
            <span>UTF-8</span>
            <span>{current.content.split("\n").length} строк</span>
            <span>{current.content.length} символов</span>
            <span className="editor-status-spacer" />
            <span className="muted">
              {isDirty ? "есть изменения — Ctrl+S, чтобы сохранить" : "сохранено"}
            </span>
          </div>
        </div>
      ) : null}
    </div>
  );
}

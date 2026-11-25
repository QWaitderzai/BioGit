import type { KeyboardEvent } from "react";
import { formatSize } from "../api";

/**
 * Встроенный редактор файлов в стиле VSCode:
 * вкладки открытых файлов, индикатор несохранённых изменений (точка на вкладке
 * и в заголовке), сохранение по Ctrl+S или кнопкой, статусная строка.
 * Двоичные файлы открываются только для просмотра метаданных.
 */

export interface OpenTab {
  /** Относительный путь файла — идентификатор вкладки. */
  name: string;
  /** Текст в редакторе (может расходиться с сохранённым). */
  content: string;
  /** Содержимое на диске на момент открытия / последнего сохранения. */
  saved: string;
  /** Двоичный файл: редактирование недоступно. */
  binary: boolean;
  /** Размер файла на диске, байт. */
  size: number;
}

interface Props {
  tabs: OpenTab[];
  active: string | null;
  busy: boolean;
  sidebarOpen: boolean;
  onSelect: (name: string) => void;
  onClose: (name: string) => void;
  onChange: (name: string, content: string) => void;
  onSave: (name: string) => void;
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
  tabs,
  active,
  busy,
  sidebarOpen,
  onSelect,
  onClose,
  onChange,
  onSave,
  onToggleSidebar,
}: Props) {
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
          const dirty = tab.content !== tab.saved;
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
              Изменённый файл можно сохранить кнопкой или сочетанием{" "}
              <code>Ctrl+S</code>, а затем зафиксировать снимок внизу экрана.
            </p>
          </div>
        </div>
      ) : current.binary ? (
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
      ) : (
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
      )}
    </div>
  );
}

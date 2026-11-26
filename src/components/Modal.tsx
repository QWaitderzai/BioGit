import {
  createContext,
  useCallback,
  useContext,
  useRef,
  useState,
  type ReactNode,
} from "react";

// Модальные диалоги вместо window.confirm: промис-функция confirm(),
// вызываемая из любого места через useConfirm().

interface ConfirmOptions {
  title: string;
  message: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
}

type ConfirmFn = (options: ConfirmOptions) => Promise<boolean>;

const ConfirmContext = createContext<ConfirmFn | null>(null);

interface ActiveDialog extends ConfirmOptions {
  resolve: (value: boolean) => void;
}

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const [dialog, setDialog] = useState<ActiveDialog | null>(null);
  const seq = useRef(0);

  const confirm = useCallback<ConfirmFn>((options) => {
    return new Promise<boolean>((resolve) => {
      seq.current += 1;
      setDialog({ ...options, resolve });
    });
  }, []);

  const settle = (value: boolean) => {
    if (dialog) {
      dialog.resolve(value);
      setDialog(null);
    }
  };

  return (
    <ConfirmContext.Provider value={confirm}>
      {children}
      {dialog && (
        <div className="modal-overlay" role="dialog" aria-modal="true">
          <div className="modal-card">
            <h3 className="modal-title">{dialog.title}</h3>
            <div className="modal-message">{dialog.message}</div>
            <div className="modal-actions">
              <button
                type="button"
                className="btn btn-ghost"
                onClick={() => settle(false)}
                autoFocus
              >
                {dialog.cancelLabel ?? "Отмена"}
              </button>
              <button
                type="button"
                className={`btn ${dialog.danger ? "btn-danger" : "btn-primary"}`}
                onClick={() => settle(true)}
              >
                {dialog.confirmLabel ?? "Подтвердить"}
              </button>
            </div>
          </div>
        </div>
      )}
    </ConfirmContext.Provider>
  );
}

/** Хук доступа к системным диалогам подтверждения. */
export function useConfirm(): ConfirmFn {
  const ctx = useContext(ConfirmContext);
  if (!ctx) {
    // на случай использования вне провайдера — откат к window.confirm
    return (options) => Promise.resolve(window.confirm(`${options.title}\n\n${typeof options.message === "string" ? options.message : ""}`));
  }
  return ctx;
}

/** Универсальное модальное окно (для diff, resolve-панелей и т.п.). */
export function Modal({
  title,
  onClose,
  children,
  wide,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  wide?: boolean;
}) {
  return (
    <div className="modal-overlay" role="dialog" aria-modal="true">
      <div className={`modal-card modal-card-content ${wide ? "modal-wide" : ""}`}>
        <div className="modal-head">
          <h3 className="modal-title">{title}</h3>
          <button
            type="button"
            className="tab-close"
            title="Закрыть"
            onClick={onClose}
          >
            ×
          </button>
        </div>
        <div className="modal-body">{children}</div>
      </div>
    </div>
  );
}

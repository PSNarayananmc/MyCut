import React, { createContext, useCallback, useContext, useRef, useState } from "react";

export type ToastKind = "info" | "ok" | "bad";

interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
}

interface ToastApi {
  show: (text: string, kind?: ToastKind) => void;
}

const Ctx = createContext<ToastApi>({ show: () => undefined });

export const useToast = (): ToastApi => useContext(Ctx);

export const ToastProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const nextId = useRef(1);

  const show = useCallback((text: string, kind: ToastKind = "info") => {
    const id = nextId.current++;
    setToasts((prev) => [...prev.slice(-4), { id, kind, text }]);
    window.setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, kind === "bad" ? 7000 : 4000);
  }, []);

  return (
    <Ctx.Provider value={{ show }}>
      {children}
      <div className="toasts" role="status" aria-live="polite">
        {toasts.map((t) => (
          <div key={t.id} className={`toast ${t.kind}`}>
            {t.text}
          </div>
        ))}
      </div>
    </Ctx.Provider>
  );
};

/** Normalize any thrown value into a readable message (errors carry context). */
export const errMsg = (e: unknown): string => {
  if (e instanceof Error) return e.message;
  return String(e);
};

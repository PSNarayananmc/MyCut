import React, { useState } from "react";
import { api, type ChatTurn } from "../lib/bridge";
import { strings as t } from "../i18n/en";
import { errMsg } from "./Toast";

const AIPanel: React.FC<{
  offline: boolean;
  modelName: string | null;
  connState: "unknown" | "checking" | "ok" | "bad";
  onRefresh: () => void;
  onOpenSettings: () => void;
}> = ({ offline, modelName, connState, onRefresh, onOpenSettings }) => {
  const [turns, setTurns] = useState<ChatTurn[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);

  const send = async () => {
    const request = input.trim();
    if (!request || busy) return;
    setInput("");
    setTurns((prev) => [
      ...prev,
      { role: "user", content: request },
      { role: "assistant", content: t.chat.applying, pending: true },
    ]);
    setBusy(true);
    try {
      const d = await api.applyAiRequest(request);
      setTurns((prev) => {
        const next = [...prev];
        next[next.length - 1] = {
          role: "assistant",
          content:
            d.summary_parts.filter(Boolean).join(". ") ||
            d.clarification ||
            "No operations returned.",
        };
        return next;
      });
      onRefresh();
    } catch (e) {
      const msg = errMsg(e);
      let text = msg;
      if (msg.includes("backend-unavailable")) text = `${t.chat.offlineTitle}: ${t.chat.offlineNoKey}`;
      // Match the actionable text returned by both the Tauri shell and the
      // server crate — case-insensitive so future copy tweaks don't break it.
      if (/no.*nvidia nim api key/i.test(msg) || msg.toLowerCase().includes("no-api-key")) {
        text = `${t.errors.noKeyTitle}. ${t.errors.noKeyBody}`;
      }
      setTurns((prev) => {
        const next = [...prev];
        // Replace the pending placeholder with the error (unless a system
        // turn was already added).
        if (next.length && next[next.length - 1].pending) {
          next[next.length - 1] = { role: "system", content: text };
        } else {
          next.push({ role: "system", content: text });
        }
        return next;
      });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="chat">
      <div className="model-banner">
        <span>{t.chat.aiModel}:</span>
        <b>{modelName ?? "—"}</b>
        {connState === "ok" && <span style={{ color: "var(--ok)" }}>✓</span>}
        {connState === "bad" && (
          <button className="small ghost" onClick={onOpenSettings}>
            {t.app.settings}
          </button>
        )}
      </div>
      <div className="chat-log">
        {!turns.length && (
          <div className="msg system">
            {offline ? `${t.chat.offlineTitle}: ${t.chat.offlineNoKey}` : t.chat.placeholder}
          </div>
        )}
        {turns.map((turn, i) => (
          <div key={i} className={`msg ${turn.role}${turn.pending ? " pending" : ""}`}>
            {turn.content}
          </div>
        ))}
      </div>
      <div className="privacy">🔒 {t.chat.privacyNotice}</div>
      <div className="chat-input">
        <textarea
          value={input}
          placeholder={offline ? t.chat.offlineNoKey : t.chat.placeholder}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button className="primary" onClick={send} disabled={busy || offline}>
          {t.chat.send}
        </button>
      </div>
    </div>
  );
};

export default AIPanel;

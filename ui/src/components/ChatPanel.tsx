import { useState } from "react";
import { api, type AiDiff, type ChatTurn } from "../lib/bridge";
import { strings as t } from "../i18n/en";

const ChatPanel: React.FC<{ offline: boolean; onRefresh: () => void }> = ({ offline, onRefresh }) => {
  const [turns, setTurns] = useState<ChatTurn[]>([]);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [diff, setDiff] = useState<AiDiff | null>(null);

  const send = async () => {
    const request = input.trim();
    if (!request || busy) return;
    setInput("");
    setTurns((prev) => [...prev, { role: "user", content: request }, { role: "assistant", content: t.chat.applying, pending: true }]);
    setBusy(true);
    try {
      const d = await api.applyAiRequest(request);
      setDiff(d);
      setTurns((prev) => {
        const next = [...prev];
        next[next.length - 1] = { role: "assistant", content: d.summary_parts.join(". ") || d.clarification || "No operations returned." };
        return next;
      });
      onRefresh();
    } catch (e) {
      const msg = String(e).includes("backend-unavailable") ? t.chat.offlineNoKey : String(e);
      setTurns((prev) => [...prev, { role: "system", content: msg }]);
    } finally {
      setBusy(false);
    }
  };

  const applyPending = async () => {
    if (!diff) return;
    try {
      await api.invoke("ai_apply_pending", {});
      setDiff(null);
      onRefresh();
    } catch (e) {
      alert(String(e));
    }
  };

  return (
    <div className="chat">
      <div className="chat-log">
        {!turns.length && <div className="msg system">{offline ? `${t.chat.offlineTitle}: ${t.chat.offlineNoKey}` : t.chat.placeholder}</div>}
        {turns.map((turn, i) => (
          <div key={i} className={`msg ${turn.role}`}>
            {turn.content}
            {turn.role === "assistant" && diff && i === turns.length - 1 && diff.summary_parts.length > 3 && (
              <div className="diff">
                <strong>{t.chat.diffTitle}</strong>
                <ul>
                  {diff.summary_parts.map((s, j) => (
                    <li key={j}>{s}</li>
                  ))}
                </ul>
                <div className="diff-actions">
                  <button className="btn" onClick={applyPending}>{t.chat.apply}</button>
                  <button className="btn" onClick={() => setDiff(null)}>{t.chat.cancel}</button>
                </div>
              </div>
            )}
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
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) void send();
          }}
        />
        <button className="btn btn-primary" onClick={send} disabled={busy || offline}>
          {t.chat.send}
        </button>
      </div>
    </div>
  );
};

export default ChatPanel;

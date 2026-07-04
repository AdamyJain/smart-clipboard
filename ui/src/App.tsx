import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./App.css";

type Hit = {
  id: string;
  raw_text: string;
  entity_type: string;
  source_app: string | null;
  captured_at: number;
  score: number;
  matched_by: string;
};

type Session = {
  id: string;
  topic: string | null;
  status: string;
  started_at: number;
  last_activity_at: number;
  capture_count: number;
};

const FILTERS = [
  ["", "all"],
  ["color", "colors"],
  ["url", "urls"],
  ["code", "code"],
  ["email", "emails"],
  ["date", "dates"],
  ["filepath", "paths"],
] as const;

function ago(ts: number): string {
  const s = Math.max(0, (Date.now() - ts) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}

function SessionsView() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [caps, setCaps] = useState<Hit[]>([]);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameVal, setRenameVal] = useState("");
  const [mergeFrom, setMergeFrom] = useState<string | null>(null);

  const reload = useCallback(() => {
    invoke<Session[]>("list_sessions").then(setSessions).catch(console.error);
  }, []);
  useEffect(() => {
    reload();
    const un = listen("capture", reload);
    return () => {
      un.then((u) => u());
    };
  }, [reload]);

  const expand = (id: string) => {
    if (expanded === id) {
      setExpanded(null);
      return;
    }
    setExpanded(id);
    invoke<Hit[]>("session_captures", { sessionId: id }).then(setCaps).catch(console.error);
  };

  const deleteCapture = useCallback(async (id: string, e: React.MouseEvent) => {
    e.stopPropagation();
    await invoke("delete_capture", { captureId: id });
    setCaps((cs) => cs.filter((c) => c.id !== id));
    reload();
  }, [reload]);

  const openSessions = sessions.filter((s) => s.status === "open");

  return (
    <ul className="results sessions">
      {sessions.map((s) => (
        <li key={s.id} className="session">
          <div className="session-head" onClick={() => expand(s.id)}>
            {renaming === s.id ? (
              <input
                className="rename"
                autoFocus
                value={renameVal}
                onClick={(e) => e.stopPropagation()}
                onChange={(e) => setRenameVal(e.target.value)}
                onKeyDown={async (e) => {
                  if (e.key === "Enter") {
                    await invoke("rename_session", { sessionId: s.id, topic: renameVal });
                    setRenaming(null);
                    reload();
                  } else if (e.key === "Escape") setRenaming(null);
                }}
              />
            ) : (
              <span className="session-topic">
                {s.status === "open" ? "● " : ""}
                {s.topic || "untitled"}
              </span>
            )}
            <span className="session-meta">
              {s.capture_count} · {ago(s.last_activity_at)}
            </span>
            <span className="session-actions" onClick={(e) => e.stopPropagation()}>
              <button
                title="rename"
                onClick={() => {
                  setRenaming(s.id);
                  setRenameVal(s.topic || "");
                }}
              >
                ✎
              </button>
              {mergeFrom && mergeFrom !== s.id ? (
                <button
                  title="merge here"
                  className="merge-target"
                  onClick={async () => {
                    await invoke("merge_sessions", { fromId: mergeFrom, toId: s.id });
                    setMergeFrom(null);
                    setExpanded(null);
                    reload();
                  }}
                >
                  ⇐ merge here
                </button>
              ) : (
                <button
                  title="merge into another session"
                  onClick={() => setMergeFrom(mergeFrom === s.id ? null : s.id)}
                >
                  {mergeFrom === s.id ? "cancel" : "merge…"}
                </button>
              )}
              {s.status === "open" && (
                <button
                  title="close session"
                  onClick={async () => {
                    await invoke("close_session", { sessionId: s.id });
                    reload();
                  }}
                >
                  ✓
                </button>
              )}
            </span>
          </div>
          {expanded === s.id && (
            <ul className="session-captures">
              {caps.map((c) => (
                <li key={c.id} className="hit">
                  <div className="hit-text">{c.raw_text.slice(0, 160)}</div>
                  <div className="hit-meta">
                    <span className="badge">{c.entity_type}</span>
                    <span>{ago(c.captured_at)}</span>
                    <select
                      value=""
                      title="move to session"
                      onChange={async (e) => {
                        if (!e.target.value) return;
                        await invoke("reassign_capture", {
                          captureId: c.id,
                          toSession: e.target.value === "__none" ? null : e.target.value,
                        });
                        expand(s.id);
                        setExpanded(s.id);
                        invoke<Hit[]>("session_captures", { sessionId: s.id }).then(setCaps);
                        reload();
                      }}
                    >
                      <option value="">move…</option>
                      <option value="__none">(no session)</option>
                      {openSessions
                        .filter((o) => o.id !== s.id)
                        .map((o) => (
                          <option key={o.id} value={o.id}>
                            {o.topic || "untitled"}
                          </option>
                        ))}
                    </select>
                    <button
                      className="delete-btn"
                      title="delete capture"
                      onClick={(e) => deleteCapture(c.id, e)}
                    >
                      🗑
                    </button>
                  </div>
                </li>
              ))}
              {caps.length === 0 && <li className="empty">no captures</li>}
            </ul>
          )}
        </li>
      ))}
      {sessions.length === 0 && (
        <li className="empty">no sessions yet — Alt+C on a selection starts one</li>
      )}
    </ul>
  );
}

export default function App() {
  const [view, setView] = useState<"search" | "sessions">("search");
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);
  const [selected, setSelected] = useState(0);
  const [copied, setCopied] = useState<string | null>(null);
  const [clearConfirm, setClearConfirm] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const debounce = useRef<number>(0);
  const clearConfirmTimer = useRef<number>(0);

  const runSearch = useCallback((q: string, f: string) => {
    invoke<Hit[]>("search", { query: q, entityFilter: f || null })
      .then((r) => {
        setHits(r);
        setSelected(0);
      })
      .catch(console.error);
  }, []);

  useEffect(() => {
    window.clearTimeout(debounce.current);
    debounce.current = window.setTimeout(() => runSearch(query, filter), 120);
    return () => window.clearTimeout(debounce.current);
  }, [query, filter, runSearch]);

  useEffect(() => {
    const un1 = listen("palette-opened", () => {
      setView("search");
      setQuery("");
      setFilter("");
      inputRef.current?.focus();
      runSearch("", "");
    });
    const un2 = listen("capture", () => runSearch(query, filter));
    return () => {
      un1.then((u) => u());
      un2.then((u) => u());
    };
  }, [query, filter, runSearch]);

  const copyHit = useCallback(
    async (hit: Hit) => {
      await invoke("copy_to_clipboard", { text: hit.raw_text });
      setCopied(hit.id);
      setTimeout(() => {
        setCopied(null);
        getCurrentWindow().hide();
      }, 350);
    },
    []
  );

  const deleteHit = useCallback(async (id: string, e: React.MouseEvent) => {
    e.stopPropagation();
    await invoke("delete_capture", { captureId: id });
    setHits((hs) => hs.filter((h) => h.id !== id));
  }, []);

  const clearAll = useCallback(async () => {
    if (!clearConfirm) {
      setClearConfirm(true);
      window.clearTimeout(clearConfirmTimer.current);
      clearConfirmTimer.current = window.setTimeout(() => setClearConfirm(false), 3000);
      return;
    }
    window.clearTimeout(clearConfirmTimer.current);
    setClearConfirm(false);
    await invoke("delete_all_captures");
    setHits([]);
    emit("capture", {});
  }, [clearConfirm]);

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelected((s) => Math.min(s + 1, hits.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelected((s) => Math.max(s - 1, 0));
    } else if (e.key === "Enter" && hits[selected]) {
      copyHit(hits[selected]);
    } else if (e.key === "Escape") {
      getCurrentWindow().hide();
    }
  };

  return (
    <div className="palette" onKeyDown={view === "search" ? onKey : undefined}>
      <div className="tabs">
        <div className="tab-group">
          <button
            className={view === "search" ? "tab active" : "tab"}
            onClick={() => setView("search")}
          >
            search
          </button>
          <button
            className={view === "sessions" ? "tab active" : "tab"}
            onClick={() => setView("sessions")}
          >
            sessions
          </button>
        </div>
        <button
          className={clearConfirm ? "clear-all confirm" : "clear-all"}
          title="delete all captures"
          onClick={clearAll}
        >
          {clearConfirm ? "confirm clear all?" : "clear all"}
        </button>
      </div>
      {view === "search" ? (
        <>
          <input
            ref={inputRef}
            autoFocus
            placeholder="Search your clipboard… (Enter copies, Esc closes)"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <div className="chips">
            {FILTERS.map(([val, label]) => (
              <button
                key={val}
                className={filter === val ? "chip active" : "chip"}
                onClick={() => {
                  setFilter(val);
                  inputRef.current?.focus();
                }}
              >
                {label}
              </button>
            ))}
          </div>
          <ul className="results">
            {hits.map((h, i) => (
              <li
                key={h.id}
                className={i === selected ? "hit selected" : "hit"}
                onMouseEnter={() => setSelected(i)}
                onClick={() => copyHit(h)}
              >
                <div className="hit-text">
                  {copied === h.id ? "✓ copied" : h.raw_text.slice(0, 200)}
                </div>
                <div className="hit-meta">
                  <span className="badge">{h.entity_type}</span>
                  {h.source_app && <span>{h.source_app}</span>}
                  <span>{ago(h.captured_at)}</span>
                  {h.matched_by !== "recent" && <span className="matched">{h.matched_by}</span>}
                  <button
                    className="delete-btn"
                    title="delete capture"
                    onClick={(e) => deleteHit(h.id, e)}
                  >
                    🗑
                  </button>
                </div>
              </li>
            ))}
            {hits.length === 0 && <li className="empty">nothing yet — copy something</li>}
          </ul>
        </>
      ) : (
        <SessionsView />
      )}
    </div>
  );
}

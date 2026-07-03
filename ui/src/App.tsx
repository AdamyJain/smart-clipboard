import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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

export default function App() {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);
  const [selected, setSelected] = useState(0);
  const [copied, setCopied] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const debounce = useRef<number>(0);

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
    <div className="palette" onKeyDown={onKey}>
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
            </div>
          </li>
        ))}
        {hits.length === 0 && <li className="empty">nothing yet — copy something</li>}
      </ul>
    </div>
  );
}

import { useState } from "react";
import type { Workspace } from "./workspace";

const GROUPS: { title: string; group: string; about: string }[] = [
  { title: "Plans", group: "plan", about: "Work, decisions held open, and what can safely start" },
  { title: "Spreadsheets", group: "sheet", about: "Numbers and formulas, including numbers that differ between worlds" },
  { title: "Reasoning", group: "reasoning", about: "What's uncertain, how likely each way is, and what's worth finding out" },
  { title: "Puzzles", group: "puzzle", about: "Choices and constraints, whose solutions are the worlds left over" },
];

/** What to open: the samples, or something of your own. */
export function SamplePicker({ ws, onOpened }: { ws: Workspace; onOpened?: () => void }) {
  const [err, setErr] = useState<string | null>(null);
  const a = ws.available;
  const open = async (f: () => Promise<void>) => {
    setErr(null);
    try {
      await f();
      onOpened?.();
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };
  return (
    <div className="picker">
      {GROUPS.map((g) => {
        const samples = (a?.samples ?? []).filter((s) => s.group === g.group);
        if (!samples.length) return null;
        return (
          <section key={g.title}>
            <h3>{g.title}</h3>
            <p className="muted small">{g.about}</p>
            <div className="picker-grid">
              {samples.map((s) => (
                <button key={s.name} className={`picker-card ${a?.current?.sample === s.name ? "current" : ""}`} onClick={() => open(() => ws.openSample(s.name))}>
                  <b>{s.title}</b>
                  <span className="muted small">{s.about}</span>
                </button>
              ))}
            </div>
          </section>
        );
      })}
      <OwnWorkspace ws={ws} open={open} />
      {err && <div className="error">{err}</div>}
    </div>
  );
}

function OwnWorkspace({ ws, open }: { ws: Workspace; open: (f: () => Promise<void>) => Promise<void> }) {
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [systems, setSystems] = useState<string[]>(["plan", "schedule"]);
  const available = ws.available?.systems ?? [];
  return (
    <section>
      <h3>Your own</h3>
      <p className="muted small">A workspace is a directory of plain files, so it can live in git.</p>
      <form
        className="own"
        onSubmit={(e) => {
          e.preventDefault();
          if (path.trim()) open(() => ws.openPath(path.trim()));
        }}
      >
        <input placeholder="Path to a workspace" value={path} onChange={(e) => setPath(e.target.value)} aria-label="Path to a workspace" />
        <button disabled={!path.trim()}>Open</button>
      </form>
      <details className="own-new">
        <summary>Make a new one</summary>
        <input placeholder="What to call it" value={name} onChange={(e) => setName(e.target.value)} aria-label="What to call it" />
        <div className="chips">
          {available.map((s) => (
            <label key={s.name} className="toggle" title={s.about}>
              <input type="checkbox" checked={systems.includes(s.name)} onChange={() => setSystems(systems.includes(s.name) ? systems.filter((x) => x !== s.name) : [...systems, s.name])} />
              {s.name}
            </label>
          ))}
        </div>
        <button className="primary" disabled={!path.trim() || !name.trim()} onClick={() => open(() => ws.create(path.trim(), name.trim(), systems))}>
          Make it at that path
        </button>
      </details>
    </section>
  );
}

export function Welcome({ ws }: { ws: Workspace }) {
  return (
    <div className="welcome">
      <h1>
        <span className="logo">◇</span> Recollect
      </h1>
      <p className="muted">A graph of ideas, with every open decision held open exactly. Start with one of these.</p>
      <SamplePicker ws={ws} />
    </div>
  );
}

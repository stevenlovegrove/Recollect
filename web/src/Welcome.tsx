import { useState } from "react";
import type { Workspace } from "./workspace";
import { askSure } from "./Ask";

const GROUPS: { title: string; group: string; about: string }[] = [
  { title: "Plans", group: "plan", about: "Work, decisions held open, and what can safely start" },
  { title: "Spreadsheets", group: "sheet", about: "Numbers and formulas, including numbers that differ between worlds" },
  { title: "Reasoning", group: "reasoning", about: "What's uncertain, how likely each way is, and what's worth finding out" },
  { title: "Puzzles", group: "puzzle", about: "Choices and constraints, whose solutions are the worlds left over" },
];

/** What to open: the samples, or something of your own. In the workspace menu your own come first,
 *  since that's where you'd go to open or make one, and the samples follow as examples. */
export function SamplePicker({ ws, onOpened, ownFirst = false }: { ws: Workspace; onOpened?: () => void; ownFirst?: boolean }) {
  const [err, setErr] = useState<string | null>(null);
  const a = ws.available;
  const open = async (f: () => Promise<void>) => {
    setErr(null);
    try {
      await f();
      onOpened?.();
    } catch (e) {
      // Closing the folder picker isn't a mistake.
      if (e instanceof DOMException && e.name === "AbortError") return;
      setErr(e instanceof Error ? e.message : String(e));
    }
  };
  const samples = GROUPS.map((g) => {
    const inGroup = (a?.samples ?? []).filter((s) => s.group === g.group);
    if (!inGroup.length) return null;
    return (
      <section key={g.title}>
        <h3>{g.title}</h3>
        <p className="muted small">{g.about}</p>
        <div className="picker-grid">
          {inGroup.map((s) => (
            <button key={s.name} className={`picker-card ${a?.current?.sample === s.name ? "current" : ""}`} onClick={() => open(() => ws.openSample(s.name))}>
              <b>{s.title}</b>
              <span className="muted small">{s.about}</span>
            </button>
          ))}
        </div>
      </section>
    );
  });
  const own = <OwnWorkspace ws={ws} open={open} />;
  return (
    <div className="picker">
      {ownFirst ? (
        <>
          {own}
          {err && <div className="error">{err}</div>}
          <hr className="menu-divider" />
          <h2 className="picker-examples">Examples</h2>
          {samples}
        </>
      ) : (
        <>
          {samples}
          {own}
          {err && <div className="error">{err}</div>}
        </>
      )}
    </div>
  );
}

type Opener = (f: () => Promise<void>) => Promise<void>;

function OwnWorkspace({ ws, open }: { ws: Workspace; open: Opener }) {
  return ws.backend?.kind === "http" ? <OnServer ws={ws} open={open} /> : <InBrowser ws={ws} open={open} />;
}

function Systems({ ws, systems, setSystems }: { ws: Workspace; systems: string[]; setSystems: (s: string[]) => void }) {
  return (
    <div className="chips">
      {(ws.available?.systems ?? []).map((s) => (
        <label key={s.name} className="toggle" title={s.about}>
          <input type="checkbox" checked={systems.includes(s.name)} onChange={() => setSystems(systems.includes(s.name) ? systems.filter((x) => x !== s.name) : [...systems, s.name])} />
          {s.name}
        </label>
      ))}
    </div>
  );
}

/** Served by `recollect serve`: a workspace is a directory it can reach by path. */
function OnServer({ ws, open }: { ws: Workspace; open: Opener }) {
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [systems, setSystems] = useState<string[]>(["plan", "schedule"]);
  const b = ws.backend!;
  return (
    <section>
      <h3>Your own</h3>
      <p className="muted small">A workspace is a directory of plain files, so it can live in git.</p>
      <form
        className="own"
        onSubmit={(e) => {
          e.preventDefault();
          if (path.trim()) open(() => b.openPath!(path.trim()));
        }}
      >
        <input placeholder="Path to a workspace" value={path} onChange={(e) => setPath(e.target.value)} aria-label="Path to a workspace" />
        <button disabled={!path.trim()}>Open</button>
      </form>
      <details className="own-new">
        <summary>Make a new one</summary>
        <input placeholder="What to call it" value={name} onChange={(e) => setName(e.target.value)} aria-label="What to call it" />
        <Systems ws={ws} systems={systems} setSystems={setSystems} />
        <button className="primary" disabled={!path.trim() || !name.trim()} onClick={() => open(() => b.createAt!(path.trim(), name.trim(), systems))}>
          Make it at that path
        </button>
      </details>
    </section>
  );
}

/** Run here: a workspace is kept in this browser, or is a folder on this computer. */
function InBrowser({ ws, open }: { ws: Workspace; open: Opener }) {
  const [name, setName] = useState("");
  const [systems, setSystems] = useState<string[]>(["plan", "schedule"]);
  const b = ws.backend;
  if (!b) return null;
  const recent = ws.available?.recent ?? [];
  const current = ws.available?.current?.id;
  return (
    <section>
      <h3>Your own</h3>
      <p className="muted small">
        {b.folders
          ? "A workspace is plain files. Keep it in this browser, or in a folder on this computer, where it can live in git and other programs, Claude among them, can work on it while you watch."
          : "A workspace is plain files, kept in this browser. This browser can't open folders, though Chrome and Edge can, so to move one in or out, use a zip."}
      </p>
      {recent.length > 0 && (
        <ul className="known">
          {recent.map((k) => (
            <li key={k.id} className={k.id === current ? "current" : ""}>
              <button className="link" onClick={() => open(() => b.openKnown!(k.id))}>
                <b>{k.name}</b>
              </button>{" "}
              <span className="muted small">{k.kind === "folder" ? `folder ${k.location}` : "in this browser"}</span>{" "}
              <button
                className="link small muted"
                title={k.kind === "folder" ? "Forget it here. The folder isn't touched." : "Delete it from this browser"}
                onClick={async () => {
                  const sure = k.kind === "folder" || (await askSure(`Delete “${k.name}” from this browser? This can't be undone.`, "Delete"));
                  if (sure) open(() => b.forget!(k.id));
                }}
              >
                {k.kind === "folder" ? "forget" : "delete"}
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="own">
        {b.folders && <button onClick={() => open(() => b.openFolder!())}>Open a folder…</button>}
        <label className="button-like">
          Import a zip…
          <input type="file" accept=".zip,application/zip" hidden onChange={(e) => e.target.files?.[0] && open(() => b.importZip!(e.target.files![0]))} />
        </label>
        {!b.folders && (
          <label className="button-like">
            Import a folder…
            <input type="file" hidden {...{ webkitdirectory: "" }} onChange={(e) => e.target.files?.length && open(() => b.importFolder!(e.target.files!))} />
          </label>
        )}
      </div>
      <details className="own-new">
        <summary>Make a new one</summary>
        <input placeholder="What to call it" value={name} onChange={(e) => setName(e.target.value)} aria-label="What to call it" />
        <Systems ws={ws} systems={systems} setSystems={setSystems} />
        <div className="row">
          <button className="primary" disabled={!name.trim()} onClick={() => open(() => b.createInBrowser!(name.trim(), systems))}>
            Keep it in this browser
          </button>
          {b.folders && (
            <button disabled={!name.trim()} onClick={() => open(() => b.createInFolder!(name.trim(), systems))}>
              Put it in a folder…
            </button>
          )}
        </div>
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

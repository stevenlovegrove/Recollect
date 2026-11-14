import { useState } from "react";
import type { Workspace } from "./workspace";
import { askSure } from "./Ask";

const GROUPS: { title: string; group: string; about: string }[] = [
  { title: "Plans", group: "plan", about: "Work, decisions held open, and what can safely start" },
  { title: "Spreadsheets", group: "sheet", about: "Numbers and formulas, including numbers that differ between worlds" },
  { title: "Uncertain numbers", group: "chance", about: "Which distribution fits which kind of thing, and what each lets you ask" },
  { title: "Reasoning", group: "reasoning", about: "What's uncertain, how likely each way is, and what's worth finding out" },
  { title: "Philosophy", group: "philosophy", about: "Claims that can't all hold, and which of them the arguments leave standing" },
  { title: "Puzzles", group: "puzzle", about: "Choices and constraints, whose solutions are the worlds left over" },
];

type Opener = (f: () => Promise<void>) => Promise<void>;

/**
 * Where a workspace comes from: a new one, one of your own, or an example. The same on the welcome
 * page and in the workspace menu.
 */
export function Start({ ws, onOpened }: { ws: Workspace; onOpened?: () => void }) {
  const [err, setErr] = useState<string | null>(null);
  const open: Opener = async (f) => {
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
  const server = ws.backend?.kind === "http";
  return (
    <div className="start">
      <div className="start-own">
        <section className="start-card">
          <h3>New</h3>
          {server ? <NewOnServer ws={ws} open={open} /> : <NewHere ws={ws} open={open} />}
        </section>
        <section className="start-card">
          <h3>Open</h3>
          {server ? <OpenOnServer ws={ws} open={open} /> : <OpenHere ws={ws} open={open} />}
        </section>
      </div>
      {err && <div className="error">{err}</div>}
      <Examples ws={ws} open={open} />
    </div>
  );
}

const NEW_HINT = "Decisions, plans and dates come along as soon as you use them.";

/** Run here: kept in this browser, or in a folder on this computer. */
function NewHere({ ws, open }: { ws: Workspace; open: Opener }) {
  const [name, setName] = useState("");
  const b = ws.backend;
  if (!b) return null;
  const ready = name.trim().length > 0;
  return (
    <form
      className="start-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (ready) open(() => b.createInBrowser!(name.trim()));
      }}
    >
      <div className="start-row">
        <input placeholder="Name it" value={name} onChange={(e) => setName(e.target.value)} aria-label="What to call it" />
        <button className="primary" disabled={!ready}>
          Create
        </button>
      </div>
      <p className="muted small">
        {b.folders ? (
          <>
            Kept in this browser, or{" "}
            <button type="button" className="link" disabled={!ready} onClick={() => open(() => b.createInFolder!(name.trim()))}>
              in a folder
            </button>{" "}
            on this computer, as plain files that git and Claude can work on too.
          </>
        ) : (
          "Kept in this browser, as plain files. Move one in or out as a zip."
        )}{" "}
        {NEW_HINT}
      </p>
    </form>
  );
}

/** Served by `recollect serve`: a workspace is a directory it can reach by path. */
function NewOnServer({ ws, open }: { ws: Workspace; open: Opener }) {
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const b = ws.backend!;
  const ready = !!name.trim() && !!path.trim();
  return (
    <form
      className="start-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (ready) open(() => b.createAt!(path.trim(), name.trim()));
      }}
    >
      <input placeholder="Name it" value={name} onChange={(e) => setName(e.target.value)} aria-label="What to call it" />
      <div className="start-row">
        <input placeholder="Where: a directory path" value={path} onChange={(e) => setPath(e.target.value)} aria-label="Where to make it" />
        <button className="primary" disabled={!ready}>
          Create
        </button>
      </div>
      <p className="muted small">A directory of plain files, so it can live in git. {NEW_HINT}</p>
    </form>
  );
}

function OpenOnServer({ ws, open }: { ws: Workspace; open: Opener }) {
  const [path, setPath] = useState("");
  const b = ws.backend!;
  return (
    <form
      className="start-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (path.trim()) open(() => b.openPath!(path.trim()));
      }}
    >
      <div className="start-row">
        <input placeholder="A workspace's directory" value={path} onChange={(e) => setPath(e.target.value)} aria-label="Path to a workspace" />
        <button disabled={!path.trim()}>Open</button>
      </div>
    </form>
  );
}

function OpenHere({ ws, open }: { ws: Workspace; open: Opener }) {
  const b = ws.backend;
  if (!b) return null;
  const recent = ws.available?.recent ?? [];
  const current = ws.available?.current?.id;
  return (
    <div className="start-form">
      {recent.length > 0 ? (
        <ul className="recent">
          {recent.map((k) => (
            <li key={k.id} className={k.id === current ? "current" : ""}>
              <button className="recent-open" onClick={() => open(() => b.openKnown!(k.id))} title={k.kind === "folder" ? `The folder ${k.location}` : "Kept in this browser"}>
                <span className="recent-name">{k.name}</span>
                <span className="muted small">{k.kind === "folder" ? k.location : "this browser"}</span>
              </button>
              <button
                className="icon recent-forget"
                aria-label={k.kind === "folder" ? `Forget ${k.name}` : `Delete ${k.name}`}
                title={k.kind === "folder" ? "Forget it here. The folder isn't touched." : "Delete it from this browser"}
                onClick={async () => {
                  const sure = k.kind === "folder" || (await askSure(`Delete “${k.name}” from this browser? This can't be undone.`, "Delete"));
                  if (sure) open(() => b.forget!(k.id));
                }}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="muted small">Nothing of your own yet.</p>
      )}
      <div className="start-row start-actions">
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
    </div>
  );
}

/** The samples, a group at a time, opening on the group of the one that's open. */
function Examples({ ws, open }: { ws: Workspace; open: Opener }) {
  const a = ws.available;
  const groups = GROUPS.filter((g) => (a?.samples ?? []).some((s) => s.group === g.group));
  const currentGroup = a?.samples.find((s) => s.name === a?.current?.sample)?.group;
  const [wanted, setWanted] = useState<string | null>(null);
  const group = groups.find((g) => g.group === (wanted ?? currentGroup)) ?? groups[0];
  if (!group) return null;
  return (
    <section className="start-examples">
      <div className="start-examples-head">
        <h3>Examples</h3>
        <div className="seg" role="tablist" aria-label="Kinds of example">
          {groups.map((g) => (
            <button key={g.group} role="tab" aria-selected={g === group} className={g === group ? "active" : ""} onClick={() => setWanted(g.group)}>
              {g.title}
            </button>
          ))}
        </div>
      </div>
      <p className="muted small">{group.about}.</p>
      <div className="picker-grid">
        {(a?.samples ?? [])
          .filter((s) => s.group === group.group)
          .map((s) => (
            <button key={s.name} className={`picker-card ${a?.current?.sample === s.name ? "current" : ""}`} onClick={() => open(() => ws.openSample(s.name))}>
              <b>{s.title}</b>
              <span className="muted small">{s.about}</span>
            </button>
          ))}
      </div>
    </section>
  );
}

export function Welcome({ ws }: { ws: Workspace }) {
  return (
    <div className="welcome">
      <header className="welcome-head">
        <h1>
          <span className="logo">◇</span> Recollect
        </h1>
        <p className="muted">A graph of ideas, with every open decision held open exactly.</p>
      </header>
      <Start ws={ws} />
    </div>
  );
}

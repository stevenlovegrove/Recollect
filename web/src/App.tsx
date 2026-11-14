import { useCallback, useEffect, useRef, useState } from "react";
import type { Op } from "./api";
import { Key, ShortcutSheet, isTyping, useHoldMeta } from "./Shortcuts";
import { useWorkspace } from "./workspace";
import type { Status } from "./backend";
import { MapView } from "./views/MapView";
import { OutlineView } from "./views/OutlineView";
import { TableView } from "./views/TableView";
import { DecisionsView } from "./views/DecisionsView";
import { WorkView } from "./views/WorkView";
import { TimelineView } from "./views/TimelineView";
import { ChecksView } from "./views/ChecksView";
import { HistoryView } from "./views/HistoryView";
import { FilesView } from "./views/FilesView";
import { EdgeInspector, Inspector, NewNodePanel, type NodeTemplate } from "./Inspector";
import { parseEdgeKey, tickOp } from "./bits";
import { ContextMenu, type CtxTarget } from "./ContextMenu";
import { Worlds, hasWorlds } from "./WorldBar";
import { ViewIcon } from "./ViewNav";
import { DdItemButton, Dropdown } from "./Dropdown";
import { Start, Welcome } from "./Welcome";
import { isDropped, label } from "./theme";
import { AskHost, askSure, askWhy } from "./Ask";
import { BottomNav } from "./ViewNav";

type Tab = "map" | "outline" | "table" | "decisions" | "work" | "timeline" | "checks" | "history" | "files";

/** Views for looking after a workspace rather than working in it: behind ⋯ at the end of the tabs. */
const MENU_TABS: Tab[] = ["files", "history", "checks"];

function hashTab(): Tab {
  return (location.hash.replace("#", "") || "map") as Tab;
}

export function App() {
  const ws = useWorkspace();
  const [wanted, setWanted] = useState<Tab>(hashTab);
  const [selected, setSelected] = useState<string | null>(null);
  const [creating, setCreating] = useState<NodeTemplate | null>(null);
  const [picking, setPicking] = useState(false);
  const newCount = useRef(0);
  const startNew = (t: Omit<NodeTemplate, "n">) => {
    setSelected(null);
    setCreating({ ...t, n: ++newCount.current });
  };
  const select = (id: string | null) => {
    setCreating(null);
    setSelected(id);
  };
  const [sheet, setSheet] = useState(false);
  // The worlds panel, open over whichever page is showing.
  const [worldsOpen, setWorldsOpen] = useState(false);
  const [ctx, setCtx] = useState<CtxTarget | null>(null);
  const [command, setCommand] = useState<{ kind: "edit" | "link"; n: number } | null>(null);
  const holdingMeta = useHoldMeta();
  const [focusDecision, setFocusDecision] = useState<{ id: string; n: number } | null>(null);
  const [focusTable, setFocusTable] = useState<{ name: string; n: number } | null>(null);
  const view = ws.view;

  // What this workspace holds makes worth looking at: decisions once there's one, work once there's
  // some, and dates once there's a task to have them.
  const holds = (c: string) => !!view?.nodes.some((n) => n.components.some((x) => x.name === c));
  const tabs: Tab[] = [
    "map",
    "outline",
    "table",
    ...(holds("decision") ? (["decisions"] as Tab[]) : []),
    ...(holds("work") ? (["work"] as Tab[]) : []),
    ...(holds("task") && view?.workspace.systems.includes("schedule") ? (["timeline"] as Tab[]) : []),
  ];
  const tab: Tab = tabs.includes(wanted) || MENU_TABS.includes(wanted) ? wanted : "map";
  const setTab = useCallback((t: Tab) => setWanted(t), []);

  // The map stays mounted once opened, so its view, filters and search survive tab changes.
  const mapOpened = useRef(false);
  if (tab === "map") mapOpened.current = true;
  const showKeys = holdingMeta || sheet;
  const showTable = (name: string) => {
    setFocusTable((f) => ({ name, n: (f?.n ?? 0) + 1 }));
    setTab("table");
  };
  const showDecisions = (id: string) => {
    setFocusDecision((f) => ({ id, n: (f?.n ?? 0) + 1 }));
    setTab("decisions");
  };

  useEffect(() => {
    if (location.hash !== `#${wanted}`) history.pushState(null, "", `#${wanted}`);
  }, [wanted]);
  useEffect(() => {
    const onHash = () => setWanted(hashTab());
    window.addEventListener("hashchange", onHash);
    window.addEventListener("popstate", onHash);
    return () => {
      window.removeEventListener("hashchange", onHash);
      window.removeEventListener("popstate", onHash);
    };
  }, []);

  // A different workspace is a different graph, so nothing selected carries over, and neither does
  // the map's view of it.
  const wsId = view?.workspace.id;
  const [mapKey, setMapKey] = useState(0);
  useEffect(() => {
    setSelected(null);
    setCreating(null);
    setPicking(false);
    setMapKey((k) => k + 1);
  }, [wsId]);

  // Drop the selection if its node or edge disappears: deleted, or edited on disk.
  const selEdge = parseEdgeKey(selected);
  useEffect(() => {
    if (!selected || !view) return;
    const e = parseEdgeKey(selected);
    const exists = e ? ws.edgeById.has(e) : ws.byId.has(selected);
    if (!exists) setSelected(null);
  }, [view, ws.byId, ws.edgeById, selected]);

  const selNode = selected ? ws.byId.get(selected) : undefined;
  const closeCtx = useCallback(() => setCtx(null), []);

  // App-wide keys. Map and Outline add their own for navigation. Resubscribing each render
  // keeps this reading current state without a long dependency list.
  useEffect(() => {
    const run = (ops: Op[], why = "") => ws.apply(ops, why).catch((e) => alert(e instanceof Error ? e.message : String(e)));
    const onKey = (e: KeyboardEvent) => {
      if (document.querySelector(".link-menu, .ctx-menu, .dd-pop, .ask")) return;
      // Undo and redo, unless a text field would undo its own typing.
      if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === "z" && !isTyping(e.target) && view) {
        e.preventDefault();
        (e.shiftKey ? ws.redo() : ws.undo()).catch((x) => alert(x instanceof Error ? x.message : String(x)));
        return;
      }
      if (e.key === "Escape") {
        if (isTyping(e.target)) (e.target as HTMLElement).blur();
        else if (sheet) setSheet(false);
        else if (picking) setPicking(false);
        else if (worldsOpen) setWorldsOpen(false);
        else if (creating) setCreating(null);
        else setSelected(null);
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey || isTyping(e.target) || !view) return;
      const key = e.key;
      const tabIndex = Number(key) - 1;
      if (key === "?") setSheet(!sheet);
      else if (key === "n") startNew({ heading: "New node" });
      else if (key === "w" && hasWorlds(ws)) setWorldsOpen(!worldsOpen);
      else if (tabIndex >= 0 && tabIndex < tabs.length && key !== "0") setTab(tabs[tabIndex]);
      else if (selNode && key === "c" && ws.partKind) startNew({ heading: `New part of “${selNode.name}”`, parent: selNode.id });
      else if (selNode && key === "s") {
        const up = view.edges.find((l) => l.from === selNode.id && ws.partKind && l.components.includes(ws.partKind));
        if (up) startNew({ heading: `New sibling of “${selNode.name}”`, parent: up.to });
        else startNew({ heading: "New node" });
      } else if (selNode && (key === "e" || key === "Enter")) {
        // Enter on a focused button or link means that control, not the selection.
        if (key === "Enter" && e.target instanceof Element && e.target.closest("button, a")) return;
        setCommand((c) => ({ kind: "edit", n: (c?.n ?? 0) + 1 }));
      } else if (selNode && key === "l") setCommand((c) => ({ kind: "link", n: (c?.n ?? 0) + 1 }));
      else if (selNode?.cells.done && key === "x") {
        // A node's own formula decides for itself, as it does for the box.
        if (!(selNode.cells.done.source === "own" && selNode.cells.done.formula !== undefined)) run([tickOp(selNode.id, "done", selNode.cells.done)]);
      } else if (selNode?.cells.dropped && (key === "Backspace" || key === "Delete")) {
        if (isDropped(selNode)) run([{ op: "set", id: selNode.id, cell: "dropped" }], "brought back");
        else {
          askWhy(`Drop “${selNode.name}”?`, "Drop").then((why) => {
            if (why !== null) run([{ op: "set", id: selNode.id, cell: "dropped", value: true }], why);
          });
        }
      } else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  if (ws.nothingOpen) return <Welcome ws={ws} />;
  if (!view) {
    return <div className="splash">{ws.error ? (ws.backend?.kind === "http" ? `Can't reach the Recollect server: ${ws.error}` : ws.error) : "Loading…"}</div>;
  }

  const open = ws.plan.decisions.filter((m) => !m.chosen);
  const blocking = open.filter((m) => m.pressure.state === "blocking").length;
  const broken = view.checks.filter((c) => c.severity === "broken").length;
  const counts: Partial<Record<Tab, number>> = {
    decisions: blocking || open.length,
    checks: view.checks.length,
    work: ws.plan.work.filter((w) => w.standing === "now").length,
  };
  const hot = (t: Tab) => (t === "decisions" && blocking > 0) || (t === "checks" && broken > 0);
  const menuCount = MENU_TABS.filter(hot).reduce((n, t) => n + (counts[t] ?? 0), 0);

  const body = (() => {
    const common = { ws, selected, onSelect: select };
    switch (tab) {
      case "map":
        return null; // Rendered separately below, to stay alive across tabs.
      case "outline":
        return <OutlineView {...common} onShowDecisions={showDecisions} onContext={setCtx} />;
      case "table":
        return <TableView {...common} focus={focusTable} />;
      case "decisions":
        return <DecisionsView {...common} focus={focusDecision} />;
      case "work":
        return <WorkView {...common} />;
      case "timeline":
        return <TimelineView {...common} />;
      case "checks":
        return <ChecksView {...common} />;
      case "history":
        return <HistoryView {...common} />;
      case "files":
        return <FilesView key={wsId} {...common} />;
    }
  })();

  return (
    <div className={`app ${showKeys ? "show-keys" : ""}`}>
      <header className="topbar">
        <div className="brand">
          <span className="logo">◇</span> <span className="brand-name">Recollect</span>{" "}
          <button className="ws-name" onClick={() => setPicking(!picking)} title={view.workspace.location}>
            <span className="brand-slash">/ </span>
            <span className="ws-name-text">{view.workspace.name}</span> ▾
          </button>
        </div>
        <nav className="tabs">
          {tabs.map((t, i) => (
            <button key={t} className={t === tab ? "tab active key-host" : "tab key-host"} onClick={() => setTab(t)}>
              <Key k={String(i + 1)} />
              {label(t)}
              {counts[t] ? <span className={hot(t) ? "badge hot" : "badge"}>{counts[t]}</span> : null}
            </button>
          ))}
        </nav>
        <span className={`tabs-more ${MENU_TABS.includes(tab) ? "active" : ""}`} title="Files, history and checks">
            {/* Something broken can only come from a hand edit, and shouldn't go unseen behind a menu. */}
            <Dropdown label={<>{MENU_TABS.includes(tab) ? label(tab) : "⋯"}{menuCount > 0 && <span className="badge hot">{menuCount}</span>}</>} active={MENU_TABS.includes(tab)}>
              {MENU_TABS.map((t) => (
                <DdItemButton key={t} onClick={() => setTab(t)}>
                  <ViewIcon name={t} />
                  <span className="dd-label">{label(t)}</span>
                  {counts[t] ? <span className={hot(t) ? "badge hot" : "badge"}>{counts[t]}</span> : null}
                </DdItemButton>
              ))}
            </Dropdown>
          </span>
        <div className="topbar-right">
          <button className="primary key-host new-node" onClick={() => startNew({ heading: "New node" })} title="New node (n)" aria-label="New node">
            <Key k="n" />+<span className="new-node-word"> Node</span>
          </button>
          <SaveDot status={ws.status} />
        </div>
      </header>
      {ws.status.readOnly && <div className="at-bar">This workspace is open in another tab, so it can only be looked at here. Close it there, and this tab takes over.</div>}
      {ws.status.state === "failed" && <div className="at-bar error">{ws.status.error} It'll be tried again with the next change.</div>}
      {/* On the map the worlds chip floats over it instead. */}
      {tab !== "map" && <Worlds ws={ws} open={worldsOpen} onOpen={setWorldsOpen} onSelect={select} />}
      {ws.error && <div className="error about-error">{ws.error}</div>}
      <main className="main">
        <section className="view">
          {mapOpened.current && (
            <div className={tab === "map" ? "map-layer" : "map-layer hidden"}>
              <MapView key={mapKey} ws={ws} selected={selected} onSelect={select} active={tab === "map"} onContext={setCtx} onShowTable={showTable} worldsOpen={worldsOpen} onWorldsOpen={setWorldsOpen} />
            </div>
          )}
          {body}
        </section>
        {creating ? (
          <NewNodePanel key={`new-${creating.n}`} ws={ws} template={creating} onCreated={(id) => select(id)} onClose={() => setCreating(null)} />
        ) : (
          selected && ws.byId.has(selected) && <Inspector key={selected} ws={ws} id={selected} onSelect={select} onClose={() => select(null)} onAdd={startNew} onShowDecisions={showDecisions} onShowTable={showTable} command={command} />
        )}
        {selEdge && <EdgeInspector key={selected} ws={ws} edge={selEdge} onSelect={select} onClose={() => select(null)} />}
      </main>
      {/* On a phone the inspector covers the view, so going to a view puts it away. */}
      <BottomNav
        tabs={[...tabs, ...MENU_TABS]}
        tab={tab}
        onTab={(t) => {
          setTab(t);
          select(null);
        }}
        counts={counts}
        hot={hot}
      />
      {picking && (
        <div className="modal-backdrop" onClick={() => setPicking(false)}>
          <div className="modal picker-modal" onClick={(e) => e.stopPropagation()}>
            {/* What's open, and what can be done with it, above where another comes from. */}
            <header className="ws-current">
              <div className="ws-current-text">
                <span className="muted small">{view.workspace.sample ? "Example, your own working copy" : ws.backend?.kind === "local" && view.workspace.location === "this browser" ? "In this browser" : view.workspace.location}</span>
                <h2>{view.workspace.name}</h2>
                {view.workspace.about && <p className="ws-about">{view.workspace.about}</p>}
              </div>
              <div className="ws-current-actions">
                {ws.backend?.exportZip && (
                  <button onClick={() => ws.backend!.exportZip!().catch((e) => alert(String(e)))} title="Save a copy as a zip">
                    Save as zip
                  </button>
                )}
                {view.workspace.sample && (
                  <button
                    title="The original is safe: this discards what you've done to your copy"
                    onClick={async () => {
                      if (await askSure(`Start “${view.workspace.name}” again? This discards what you've done to it.`, "Start again")) ws.openSample(view.workspace.sample!, true).catch((e) => alert(String(e)));
                    }}
                  >
                    Start again
                  </button>
                )}
              </div>
              <button className="icon ws-current-close" onClick={() => setPicking(false)} aria-label="Close">
                ×
              </button>
            </header>
            <Start ws={ws} onOpened={() => setPicking(false)} />
          </div>
        </div>
      )}
      {ctx && <ContextMenu key={JSON.stringify(ctx)} ws={ws} target={ctx} selected={selected} onSelect={select} onClose={closeCtx} />}
      {showKeys && <ShortcutSheet tab={tab} hasSelection={!!selNode} sticky={sheet} onClose={() => setSheet(false)} />}
      <AskHost />
    </div>
  );
}

/** Whether what's on screen is safe: saved, or connected to the server that saves it. */
function SaveDot({ status }: { status: Status }) {
  const [cls, title] = {
    live: ["dot live", "Live"],
    offline: ["dot", "Disconnected"],
    saved: ["dot live", "Saved"],
    saving: ["dot saving", "Saving…"],
    failed: ["dot failed", status.error ?? "Couldn't save"],
  }[status.state];
  return <span className={cls} title={status.readOnly ? `${title}, and only looking: it's open in another tab` : title} />;
}

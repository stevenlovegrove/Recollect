import { useCallback, useEffect, useRef, useState } from "react";
import type { Op } from "./api";
import { Key, ShortcutSheet, isTyping, useHoldMeta } from "./Shortcuts";
import { useWorkspace } from "./workspace";
import { MapView } from "./views/MapView";
import { OutlineView } from "./views/OutlineView";
import { TableView } from "./views/TableView";
import { WorldsView } from "./views/WorldsView";
import { DecisionsView } from "./views/DecisionsView";
import { WorkView } from "./views/WorkView";
import { TimelineView } from "./views/TimelineView";
import { ChecksView } from "./views/ChecksView";
import { HistoryView } from "./views/HistoryView";
import { InboxView } from "./views/InboxView";
import { FilesView } from "./views/FilesView";
import { Inspector, LinkInspector, NewNodePanel, type NodeTemplate } from "./Inspector";
import { parseEdgeKey } from "./bits";
import { ContextMenu, type CtxTarget } from "./ContextMenu";
import { WorldBar } from "./WorldBar";
import { SamplePicker, Welcome } from "./Welcome";
import { isDropped, label } from "./theme";
import { AskHost, askSure, askWhy } from "./Ask";

type Tab = "map" | "outline" | "table" | "worlds" | "decisions" | "work" | "timeline" | "checks" | "history" | "inbox" | "files";

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
  const [ctx, setCtx] = useState<CtxTarget | null>(null);
  const [command, setCommand] = useState<{ kind: "edit" | "link"; n: number } | null>(null);
  const holdingMeta = useHoldMeta();
  const [focusDecision, setFocusDecision] = useState<{ id: string; n: number } | null>(null);
  const view = ws.view;

  // What this workspace's systems make worth looking at.
  const systems = view?.workspace.systems ?? [];
  const tabs: Tab[] = [
    "map",
    "outline",
    "table",
    ...(view?.worlds.vars.length ? (["worlds"] as Tab[]) : []),
    ...(systems.includes("plan") ? (["decisions", "work"] as Tab[]) : []),
    ...(systems.includes("schedule") ? (["timeline"] as Tab[]) : []),
    "checks",
    "history",
    "inbox",
    "files",
  ];
  const tab: Tab = tabs.includes(wanted) ? wanted : "map";
  const setTab = useCallback((t: Tab) => setWanted(t), []);

  // The map stays mounted once opened, so its view, filters and search survive tab changes.
  const mapOpened = useRef(false);
  if (tab === "map") mapOpened.current = true;
  const showKeys = holdingMeta || sheet;
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
  const dir = view?.workspace.dir;
  const [mapKey, setMapKey] = useState(0);
  useEffect(() => {
    setSelected(null);
    setCreating(null);
    setPicking(false);
    setMapKey((k) => k + 1);
  }, [dir]);

  // Drop the selection if its node or link disappears: deleted, or edited on disk.
  const selEdge = parseEdgeKey(selected);
  useEffect(() => {
    if (!selected || !view) return;
    const e = parseEdgeKey(selected);
    const exists = e ? view.links.some((x) => x.from === e.from && x.kind === e.kind && x.to === e.to) : ws.byId.has(selected);
    if (!exists) setSelected(null);
  }, [view, ws.byId, selected]);

  const selNode = selected ? ws.byId.get(selected) : undefined;
  const closeCtx = useCallback(() => setCtx(null), []);

  // App-wide keys. Map and Outline add their own for navigation. Resubscribing each render
  // keeps this reading current state without a long dependency list.
  useEffect(() => {
    const run = (ops: Op[], why = "") => ws.apply(ops, why).catch((e) => alert(e instanceof Error ? e.message : String(e)));
    const onKey = (e: KeyboardEvent) => {
      if (document.querySelector(".link-menu, .ctx-menu, .dd-pop, .ask")) return;
      if (e.key === "Escape") {
        if (isTyping(e.target)) (e.target as HTMLElement).blur();
        else if (sheet) setSheet(false);
        else if (picking) setPicking(false);
        else if (creating) setCreating(null);
        else setSelected(null);
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey || isTyping(e.target) || !view) return;
      const key = e.key;
      const tabIndex = Number(key) - 1;
      if (key === "?") setSheet(!sheet);
      else if (key === "n") startNew({ heading: "New node" });
      else if (tabIndex >= 0 && tabIndex < tabs.length && key !== "0") setTab(tabs[tabIndex]);
      else if (selNode && key === "c" && ws.partKind) startNew({ heading: `New part of “${selNode.name}”`, parent: selNode.id });
      else if (selNode && key === "s") {
        const up = view.links.find((l) => l.from === selNode.id && l.kind === ws.partKind);
        if (up) startNew({ heading: `New sibling of “${selNode.name}”`, parent: up.to });
        else startNew({ heading: "New node" });
      } else if (selNode && (key === "e" || key === "Enter")) {
        // Enter on a focused button or link means that control, not the selection.
        if (key === "Enter" && e.target instanceof Element && e.target.closest("button, a")) return;
        setCommand((c) => ({ kind: "edit", n: (c?.n ?? 0) + 1 }));
      } else if (selNode && key === "l") setCommand((c) => ({ kind: "link", n: (c?.n ?? 0) + 1 }));
      else if (selNode && ws.info("done") && key === "x") {
        run([selNode.cells.done ? { op: "set", id: selNode.id, component: "done" } : { op: "set", id: selNode.id, component: "done", value: null }]);
      } else if (selNode && ws.info("dropped") && (key === "Backspace" || key === "Delete")) {
        if (isDropped(selNode)) run([{ op: "set", id: selNode.id, component: "dropped" }], "brought back");
        else {
          askWhy(`Drop “${selNode.name}”?`, "Drop").then((why) => {
            if (why !== null) run([{ op: "set", id: selNode.id, component: "dropped", value: null }], why);
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
    return <div className="splash">{ws.error ? `Can't reach the Recollect server: ${ws.error}` : "Loading…"}</div>;
  }

  const open = ws.plan.decisions.filter((m) => !m.chosen);
  const blocking = open.filter((m) => m.pressure.state === "blocking").length;
  const broken = view.checks.filter((c) => c.severity === "broken").length;
  const counts: Partial<Record<Tab, number>> = {
    decisions: blocking || open.length,
    checks: view.checks.length,
    work: ws.plan.work.filter((w) => w.standing === "now").length,
    inbox: ws.proposals.length,
  };
  const hot = (t: Tab) => t === "inbox" || (t === "decisions" && blocking > 0) || (t === "checks" && broken > 0);

  const body = (() => {
    const common = { ws, selected, onSelect: select };
    switch (tab) {
      case "map":
        return null; // Rendered separately below, to stay alive across tabs.
      case "outline":
        return <OutlineView {...common} onShowDecisions={showDecisions} onContext={setCtx} />;
      case "table":
        return <TableView {...common} />;
      case "worlds":
        return <WorldsView {...common} />;
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
      case "inbox":
        return <InboxView {...common} />;
      case "files":
        return <FilesView key={dir} {...common} />;
    }
  })();

  return (
    <div className={`app ${showKeys ? "show-keys" : ""}`}>
      <header className="topbar">
        <div className="brand">
          <span className="logo">◇</span> Recollect{" "}
          <button className="ws-name" onClick={() => setPicking(!picking)} title={view.workspace.dir}>
            / {view.workspace.name} ▾
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
        <div className="topbar-right">
          <button className="primary key-host" onClick={() => startNew({ heading: "New node" })} title="New node (n)">
            <Key k="n" />+ Node
          </button>
          <span className={ws.connected ? "dot live" : "dot"} title={ws.connected ? "Live" : "Disconnected"} />
        </div>
      </header>
      {view.latest !== null && (
        <div className="at-bar">
          You've gone back to event #{view.last_seq}. A change here starts a new branch, and nothing's lost.{" "}
          <button className="link" onClick={() => ws.checkout(view.latest!, "returned to the latest").catch((e) => alert(String(e)))}>
            Return to the latest (#{view.latest})
          </button>
        </div>
      )}
      <WorldBar ws={ws} onShowWorlds={() => setTab("worlds")} />
      {view.workspace.about && view.workspace.sample && <div className="about-bar muted small">{view.workspace.about}</div>}
      {ws.error && <div className="error about-error">{ws.error}</div>}
      <main className="main">
        <section className="view">
          {mapOpened.current && (
            <div className={tab === "map" ? "map-layer" : "map-layer hidden"}>
              <MapView key={mapKey} ws={ws} selected={selected} onSelect={select} active={tab === "map"} onContext={setCtx} />
            </div>
          )}
          {body}
        </section>
        {creating ? (
          <NewNodePanel key={`new-${creating.n}`} ws={ws} template={creating} onCreated={(id) => select(id)} onClose={() => setCreating(null)} />
        ) : (
          selected && ws.byId.has(selected) && <Inspector key={selected} ws={ws} id={selected} onSelect={select} onClose={() => select(null)} onAdd={startNew} onShowDecisions={showDecisions} command={command} />
        )}
        {selEdge && <LinkInspector key={selected} ws={ws} edge={selEdge} onSelect={select} onClose={() => select(null)} />}
      </main>
      {picking && (
        <div className="modal-backdrop" onClick={() => setPicking(false)}>
          <div className="modal picker-modal" onClick={(e) => e.stopPropagation()}>
            <div className="row spread">
              <h2>Open a workspace</h2>
              <button className="icon" onClick={() => setPicking(false)}>
                ×
              </button>
            </div>
            {view.workspace.sample && (
              <p className="small">
                <span className="muted">This is your working copy of a sample, so the original is safe.</span>{" "}
                <button
                  className="link"
                  onClick={async () => {
                    if (await askSure(`Start “${view.workspace.name}” again? This discards what you've done to it.`, "Start again")) ws.openSample(view.workspace.sample!, true).catch((e) => alert(String(e)));
                  }}
                >
                  Start it again
                </button>
              </p>
            )}
            <SamplePicker ws={ws} onOpened={() => setPicking(false)} />
          </div>
        </div>
      )}
      {ctx && <ContextMenu key={JSON.stringify(ctx)} ws={ws} target={ctx} selected={selected} onSelect={select} onClose={closeCtx} />}
      {showKeys && <ShortcutSheet tab={tab} hasSelection={!!selNode} sticky={sheet} onClose={() => setSheet(false)} />}
      <AskHost />
    </div>
  );
}

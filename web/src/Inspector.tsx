import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api, type Cell, type Edge, type EventView, type GraphNode, type Has, type Labels, type Op, type Written } from "./api";
import { edgesOf, labelsOf, type Workspace } from "./workspace";
import { badgesOf, compColor, isDropped, isNote, label, nodeColor } from "./theme";
import { DdItemButton, Dropdown } from "./Dropdown";
import { Brackets, CellValue, CompBadge, NodeChip, RemoveButton, Resolved, RevertButton, TickBox, casesOf, edgeText, shownText } from "./bits";
import { entryText, parseEntry } from "./cellEntry";
import { beliefLines } from "./summary";
import { Key } from "./Shortcuts";
import { OpLine } from "./views/HistoryView";
import { askSure, askWhy } from "./Ask";

/** A node about to be made: nothing exists until it's saved. */
export type NodeTemplate = { heading: string; parent?: string; n: number };

/** What a cell is on: a node, or the edge from one node to another. */
type Target = { id: string } | { from: string; to: string };

/** A textarea that grows with its content. */
function AutoText({ inputRef, ...props }: React.TextareaHTMLAttributes<HTMLTextAreaElement> & { inputRef?: React.Ref<HTMLTextAreaElement | null> }) {
  const own = useRef<HTMLTextAreaElement | null>(null);
  useLayoutEffect(() => {
    const el = own.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight + 2}px`;
  }, [props.value]);
  return (
    <textarea
      {...props}
      rows={1}
      ref={(el) => {
        own.current = el;
        if (typeof inputRef === "function") inputRef(el);
        else if (inputRef) (inputRef as React.RefObject<HTMLTextAreaElement | null>).current = el;
      }}
    />
  );
}

/** Edits are local until you leave the field, and follow the node if it changes underneath. */
function useDraft(value: string, commit: (v: string) => void) {
  const [draft, setDraft] = useState(value);
  const [base, setBase] = useState(value);
  if (base !== value) {
    setBase(value);
    setDraft(value);
  }
  return {
    value: draft,
    onChange: (e: React.ChangeEvent<HTMLTextAreaElement | HTMLInputElement>) => setDraft(e.target.value),
    onBlur: () => {
      if (draft !== value) commit(draft);
    },
  };
}

/** The components something can be given here: those of this place that it doesn't have, and every node doesn't have anyway. */
function addable(ws: Workspace, place: "node" | "edge", has: string[]) {
  return ws.view!.components.filter((c) => c.place === place && !c.everywhere && !has.includes(c.name));
}

export function NewNodePanel({ ws, template, onCreated, onClose }: { ws: Workspace; template: NodeTemplate; onCreated: (id: string) => void; onClose: () => void }) {
  const [name, setName] = useState("");
  const [body, setBody] = useState("");
  const [components, setComponents] = useState<string[]>(() => (template.parent && ws.info("task") ? ["task"] : []));
  const [err, setErr] = useState<string | null>(null);
  const available = addable(ws, "node", []);
  const link = ws.partKind;

  const save = async (again: boolean) => {
    const op: Op = { op: "add_node", id: "new:node", name: name.trim() || "Untitled", components, links: template.parent && link ? [{ to: template.parent, component: link }] : [] };
    if (body.trim()) op.body = body.trimEnd();
    try {
      const created = await ws.apply([op]);
      if (again) {
        setName("");
        setBody("");
      } else onCreated(created["new:node"]);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <aside className="inspector">
      <div className="insp-head">
        <div className="insp-kind">
          <span className="insp-new">{template.heading}</span>
        </div>
        <button className="icon" onClick={onClose} title="Close (esc)">
          ×
        </button>
      </div>
      <div className="insp-form">
        <AutoText
          autoFocus
          className="insp-title-input"
          aria-label="Name"
          placeholder="Name"
          value={name}
          onChange={(e) => setName(e.target.value.replace(/\n/g, " "))}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              save(e.shiftKey);
            }
          }}
        />
        <AutoText className="insp-notes" aria-label="Notes" placeholder="Notes…" value={body} onChange={(e) => setBody(e.target.value)} />
      </div>
      {template.parent && (
        <div className="insp-new-link small">
          <span className="muted">Part of</span> <NodeChip ws={ws} id={template.parent} onSelect={() => {}} />
        </div>
      )}
      <section>
        <h4>What it is</h4>
        <div className="chips">
          {available.map((c) => (
            <label key={c.name} className="toggle" title={c.about}>
              <input type="checkbox" checked={components.includes(c.name)} onChange={() => setComponents(components.includes(c.name) ? components.filter((f) => f !== c.name) : [...components, c.name])} />
              {c.icon ? `${c.icon} ` : ""}
              {label(c.name)}
            </label>
          ))}
        </div>
      </section>
      {err && <div className="error">{err}</div>}
      <div className="row end">
        <button onClick={onClose}>Cancel</button>
        <button className="primary" onClick={() => save(false)} title="⏎">
          Add
        </button>
      </div>
      <p className="form-keys muted small">
        <kbd>⏎</kbd> add · <kbd>⇧⏎</kbd> add, then another · <kbd>esc</kbd> cancel
      </p>
    </aside>
  );
}

/** Which component cards are folded away, remembered per component, so the same kind of card stays the same way. */
function useCollapsed(component: string): [boolean, () => void] {
  const key = `recollect.collapsed.${component}`;
  const read = () => {
    try {
      return localStorage.getItem(key) === "1";
    } catch {
      return false;
    }
  };
  const [collapsed, setCollapsed] = useState(read);
  return [
    collapsed,
    () => {
      const next = !collapsed;
      setCollapsed(next);
      try {
        localStorage.setItem(key, next ? "1" : "0");
      } catch {
        // Not remembered, then.
      }
    },
  ];
}

export function Inspector({
  ws,
  id,
  onSelect,
  onClose,
  onAdd,
  onShowDecisions,
  command,
}: {
  ws: Workspace;
  id: string;
  onSelect: (id: string) => void;
  onClose: () => void;
  onAdd: (t: Omit<NodeTemplate, "n">) => void;
  onShowDecisions: (id: string) => void;
  /** Keyboard requests from the app; `n` changes each time one is made. */
  command?: { kind: "edit" | "link"; n: number } | null;
}) {
  const node = ws.byId.get(id)!;
  const view = ws.view!;
  const [err, setErr] = useState<string | null>(null);
  const [linking, setLinking] = useState(false);
  const [naming, setNaming] = useState(false);
  const nameRef = useRef<HTMLTextAreaElement | null>(null);
  const lastCommand = useRef(command?.n);
  useEffect(() => {
    if (!command || command.n === lastCommand.current) return;
    lastCommand.current = command.n;
    if (command.kind === "edit") {
      nameRef.current?.focus();
      nameRef.current?.setSelectionRange(nameRef.current.value.length, nameRef.current.value.length);
    } else setLinking(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [command?.n]);

  const run = async (ops: Op[], why = "") => {
    try {
      setErr(null);
      await ws.apply(ops, why);
      return true;
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
      return false;
    }
  };

  const nameValue = typeof node.cells.name?.value === "string" ? (node.cells.name.value as string) : "";
  const name = useDraft(nameValue, (v) => v.trim() && run([{ op: "set", id, cell: "name", value: v.trim() }]));
  const { outgoing, incoming } = edgesOf(view, id, ws.ends);
  // The workspace's own cells, grouped by where they come from: a kind, or here.
  const free = Object.entries(node.cells).filter(([c]) => !ws.cellInfo(c) && c !== "name");
  const homes = new Map<string, [string, Cell][]>();
  for (const [c, cell] of free) {
    const home = cell.kind ?? cell.beneath ?? "";
    homes.set(home, [...(homes.get(home) ?? []), [c, cell]]);
  }
  // What it was given first, then what its kinds give it, then what its edges bring, then what those require.
  const order = ["given", "kind", "everywhere", "edge", "requires"];
  const shown = node.components.filter((c) => c.name !== "named").sort((a, b) => order.indexOf(a.why) - order.indexOf(b.why));
  const has = node.components.map((c) => c.name);

  return (
    <aside className="inspector">
      <div className="insp-head" style={{ borderColor: nodeColor(node) }}>
        <div className="insp-kind">
          {badgesOf(node).map((c) => (
            <CompBadge key={c} ws={ws} c={c} onClick={() => document.getElementById(`card-${c}`)?.scrollIntoView({ block: "nearest", behavior: "smooth" })} />
          ))}
          {isNote(node) && <span className="muted">note</span>}
        </div>
        <button className="icon" onClick={onClose} title="Close (esc)">
          ×
        </button>
      </div>

      <div className="insp-form">
        <div className="key-host">
          <Key k="e" />
          <AutoText inputRef={nameRef} className="insp-title-input" aria-label="Name" placeholder="Name" {...name} onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()} />
        </div>
        <code className="muted small">{id}</code>
      </div>

      {err && <div className="error">{err}</div>}

      {shown.map((c) => (
        <ComponentCard key={c.name} ws={ws} node={node} has={c} run={run} onSelect={onSelect} onShowDecisions={onShowDecisions} />
      ))}
      {[...homes].map(([home, cells]) => (
        <CellsCard key={home || "own"} ws={ws} id={id} home={home} cells={cells} onSelect={onSelect} />
      ))}
      {naming && <NewCell ws={ws} taken={Object.keys(node.cells)} onName={(c) => run([{ op: "set", id, cell: c, value: 0 }]).then(() => setNaming(false))} onCancel={() => setNaming(false)} />}

      <div className="row">
        <Dropdown label={<>＋ Add</>}>
          {addable(ws, "node", has).map((c) => (
            <DdItemButton key={c.name} color={compColor(c.name)} onClick={() => run([{ op: "add", id, component: c.name }])}>
              <span className="dd-glyph">{c.icon ?? "·"}</span>
              <span>
                <b>{label(c.name)}</b>
                <span className="dd-desc">{c.about}</span>
              </span>
            </DdItemButton>
          ))}
          <DdItemButton color="var(--c-none)" onClick={() => setNaming(true)}>
            <span className="dd-glyph">＋</span>
            <span>
              <b>A cell of its own…</b>
              <span className="dd-desc">Such as a price or a range: a value, or a formula. Give a kind one, and everything of that kind has it.</span>
            </span>
          </DdItemButton>
        </Dropdown>
      </div>

      <EdgeList title="Goes to" id={id} edges={outgoing} ws={ws} onSelect={onSelect} run={run} />
      <EdgeList title="Comes from" id={id} edges={incoming} ws={ws} onSelect={onSelect} run={run} />

      <div className="row">
        <button className="key-host" onClick={() => setLinking(!linking)}>
          <Key k="l" />
          Link…
        </button>
        {ws.partKind && (
          <button className="key-host" onClick={() => onAdd({ heading: `New part of “${node.name}”`, parent: id })}>
            <Key k="c" />
            Add part…
          </button>
        )}
      </div>
      {linking && <LinkForm ws={ws} id={id} onDone={() => setLinking(false)} run={run} />}

      <NodeHistory ws={ws} id={id} onSelect={onSelect} />

      <section className="danger-zone">
        {node.cells.dropped && (
          <button
            className="key-host"
            onClick={async () => {
              if (isDropped(node)) run([{ op: "set", id, cell: "dropped" }], "brought back");
              else {
                const why = await askWhy(`Drop “${node.name}”?`, "Drop");
                if (why !== null) run([{ op: "set", id, cell: "dropped", value: true }], why);
              }
            }}
          >
            <Key k="⌫" />
            {isDropped(node) ? "Bring back" : "Drop…"}
          </button>
        )}
        <button
          className="danger"
          onClick={async () => {
            if (await askSure(`Delete “${node.name}” and its edges? It stays in the log, and can be undone from there.`, "Delete")) run([{ op: "delete_node", id }], "deleted");
          }}
        >
          Delete
        </button>
      </section>
    </aside>
  );
}

/** Why something has a component, in a few words. */
function Why({ ws, has, self, onSelect }: { ws: Workspace; has: Has; self: string; onSelect: (id: string) => void }) {
  switch (has.why) {
    case "kind":
      return (
        <span className="comp-origin">
          from <NodeChip ws={ws} id={has.from!} onSelect={onSelect} />
        </span>
      );
    case "requires":
      // A component is a noun for what the node is, "a decision", unless it's a plural for what it has, "options".
      return <span className="comp-origin">{/[^s]s$/.test(has.from!) ? `because it has ${label(has.from!).toLowerCase()}` : `because it's ${article(has.from!)}`}</span>;
    case "edge": {
      const e = ws.edgeById.get(has.from!);
      if (!e) return null;
      // The edge's component that brings this one.
      const by = e.components.find((c) => (e.from === self ? ws.info(c)?.source : ws.info(c)?.target)?.includes(has.name)) ?? e.components[0];
      const words = relation(by);
      return e.from === self ? (
        <span className="comp-origin">
          as {words} <NodeChip ws={ws} id={e.to} onSelect={onSelect} />
        </span>
      ) : (
        <span className="comp-origin">
          as what <NodeChip ws={ws} id={e.from} onSelect={onSelect} /> is {words}
        </span>
      );
    }
    default:
      return null;
  }
}

/** An edge component as the words between its ends: "part of", "an alternative of". */
function relation(c: string): string {
  const words = label(c).toLowerCase();
  return /\b(of|on|a|to)$/.test(words) ? words : `${/^[aeiou]/.test(words) ? "an" : "a"} ${words} of`;
}

/** A component's name after "a" or "an". */
function article(c: string): string {
  const word = label(c).toLowerCase();
  return /^[aeiou]/.test(word) ? `an ${word}` : `a ${word}`;
}

/** A component and its cells, with why it's here. One given here can be taken away. */
function ComponentCard({ ws, node, has, run, onSelect, onShowDecisions }: { ws: Workspace; node: GraphNode; has: Has; run: (ops: Op[], why?: string) => Promise<boolean>; onSelect: (id: string) => void; onShowDecisions: (id: string) => void }) {
  const c = has.name;
  const info = ws.info(c);
  const [collapsed, toggle] = useCollapsed(c);
  const id = node.id;
  // A tick with nothing to say, such as done on something with nothing to finish, isn't worth showing.
  const silent = (x: string) => !!ws.cellInfo(x)?.truth && node.cells[x].source !== "own" && casesOf(node.cells[x]).every((v) => v === null);
  const cells = (info?.cells ?? []).filter((x) => node.cells[x] && !ws.cellInfo(x)?.internal && (!ws.cellInfo(x)?.quiet || node.cells[x].source === "own") && !silent(x));
  const beliefs = beliefLines(ws, node, true).filter((l) => l.component === c);
  // A choice is settled when what it lands on is written on it, rather than worked out.
  const chosen = node.cells.chosen;
  const settled = c === "choice" && chosen?.source === "own" && chosen.shown?.kind === "named" ? chosen.shown.items[0]?.node : undefined;
  const isDecision = node.components.some((x) => x.name === "decision");
  return (
    <div className={`comp ${collapsed ? "collapsed" : ""}`} id={`card-${c}`} style={{ ["--c" as string]: compColor(c) }}>
      <div className="comp-head">
        <button className="comp-fold" onClick={toggle} title={collapsed ? "Show" : "Fold away"} aria-expanded={!collapsed}>
          {collapsed ? "▸" : "▾"}
        </button>
        <span className="comp-name" title={info?.about}>
          {info?.icon && <span className="glyph">{info.icon}</span>} {label(c)}
        </span>
        <Why ws={ws} has={has} self={id} onSelect={onSelect} />
        {has.why === "given" && (
          <RemoveButton title={`Take ${label(c).toLowerCase()} away`} onClick={() => run([{ op: "remove", id, component: c }])} />
        )}
      </div>
      {!collapsed && (
        <div className="comp-body">
          {info?.about && !cells.length && !beliefs.length && <span className="muted small">{info.about}</span>}
          {c === "choice" && (settled || isDecision) && (
            <div className="small">
              {settled ? (
                <>
                  Settled on <NodeChip ws={ws} id={settled} onSelect={onSelect} />.{" "}
                  <button className="link" onClick={() => run([{ op: "set", id, cell: "chosen" }], "reopened")}>
                    reopen
                  </button>
                </>
              ) : (
                <span className="muted">
                  Still open: its options are what's linked to it as an alternative, which you can compare on the{" "}
                  <button className="link" onClick={() => onShowDecisions(id)}>
                    Decisions tab
                  </button>
                  .
                </span>
              )}
            </div>
          )}
          {beliefs.map((l) => (
            <div key={l.name} className="derived">
              <span className="derived-k" title={l.about}>
                {label(l.name)}
              </span>
              <span className="derived-v mono-value">{l.text}</span>
              <span />
            </div>
          ))}
          {c === "notes" && <NotesBody value={typeof node.cells.body?.value === "string" ? (node.cells.body.value as string) : ""} commit={(v) => run([{ op: "set", id, cell: "body", value: v.trimEnd() }])} />}
          {cells.filter((x) => !(x === "chosen" && settled) && !(c === "notes" && x === "body")).map((x) => (
            <CellRow key={x} ws={ws} target={{ id }} c={x} cell={node.cells[x]} onSelect={onSelect} />
          ))}
        </div>
      )}
    </div>
  );
}

/** Notes, as text that grows as it's written. */
function NotesBody({ value, commit }: { value: string; commit: (v: string) => void }) {
  const notes = useDraft(value, commit);
  return <AutoText className="insp-notes" aria-label="Notes" placeholder="Notes…" {...notes} />;
}

/** The workspace's own cells that come from one place: a kind, or the node itself. */
function CellsCard({ ws, id, home, cells, onSelect }: { ws: Workspace; id: string; home: string; cells: [string, Cell][]; onSelect: (id: string) => void }) {
  const [collapsed, toggle] = useCollapsed(home ? "kind cells" : "own cells");
  return (
    <div className={`comp ${collapsed ? "collapsed" : ""}`} style={{ ["--c" as string]: "var(--c-none)" }}>
      <div className="comp-head">
        <button className="comp-fold" onClick={toggle} title={collapsed ? "Show" : "Fold away"} aria-expanded={!collapsed}>
          {collapsed ? "▸" : "▾"}
        </button>
        {home ? (
          <span className="comp-name">
            Cells from <NodeChip ws={ws} id={home} onSelect={onSelect} />
          </span>
        ) : (
          <span className="comp-name">Its own cells</span>
        )}
      </div>
      {!collapsed && (
        <div className="comp-body">
          {cells.map(([c, cell]) => (
            <CellRow key={c} ws={ws} target={{ id }} c={c} cell={cell} onSelect={onSelect} own />
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * One cell: what it comes to, where that's from, and a way to write over it. A true-or-false cell on a node
 * is a box to tick. Taking away what's written here goes back to the kind's, or the default.
 *
 * What's written on this node has a bar down its side, and what it inherits says where from. A formula
 * written here or on a kind takes the whole width, with what it comes to set apart beneath it.
 */
function CellRow({ ws, target, c, cell, onSelect, own }: { ws: Workspace; target: Target; c: string; cell: Cell; onSelect: (id: string) => void; own?: boolean }) {
  const [editing, setEditing] = useState(false);
  const info = ws.cellInfo(c);
  const settable = !info || info.settable;
  const here = cell.source === "own";
  const inherited = cell.source === "kind" && !!cell.kind;
  const ownFormula = cell.formula !== undefined && here;
  const shownFormula = cell.formula !== undefined && (here || inherited);
  const tick = info?.truth && "id" in target;
  const back = cell.beneath ?? cell.kind;
  const clear = () => ws.apply([entryOp(target, c, parseEntry(""), undefined, ownFormula)]).catch((e) => alert(String(e)));
  const name = (
    <span className={`derived-k ${own ? "own-cell-k" : ""}`} title={info?.about ?? "What a formula calls it"}>
      {tick ? (
        <label className="tick-k">
          <TickBox ws={ws} id={(target as { id: string }).id} c={c} /> {label(c)}
        </label>
      ) : own ? (
        <code>{c}</code>
      ) : (
        label(c)
      )}
    </span>
  );
  const from = inherited && (
    <span className="cell-from" title={`Inherited: ${ws.byId.get(cell.kind!)?.name ?? "its kind"} says this, and nothing's written here`}>
      from <NodeChip ws={ws} id={cell.kind!} onSelect={onSelect} />
    </span>
  );
  const undo =
    here && (info?.computed || back || !info) ? (
      back ? (
        <RevertButton title={`Go back to ${ws.byId.get(back)?.name ?? "its kind"}’s`} onClick={clear} />
      ) : info?.computed ? (
        <RevertButton title="Go back to working this out" onClick={clear} />
      ) : (
        <RemoveButton title={`Take ${c} away`} onClick={clear} />
      )
    ) : null;
  const where = here ? "here" : inherited ? "inherited" : "";

  if (shownFormula || editing) {
    return (
      <div className={`derived stacked ${where}`}>
        <div className="cell-head">
          {name}
          {from}
          <span className="grow" />
          {undo}
        </div>
        {editing ? (
          <CellEditor ws={ws} target={target} c={c} cell={cell} onSelect={onSelect} opening onClosed={() => setEditing(false)} />
        ) : (
          <CellEditor key={cell.formula} ws={ws} target={target} c={c} cell={cell} onSelect={onSelect} />
        )}
      </div>
    );
  }
  return (
    <div className={`derived ${where}`}>
      {name}
      <span className="derived-v">
        <span className="cell-line">
          <CellValue ws={ws} cell={cell} onSelect={onSelect} component={c} />
          {from}
          {settable && (
            <button className="link small" title={cell.formula ? `Worked out by =${cell.formula}` : undefined} onClick={() => setEditing(true)}>
              edit
            </button>
          )}
        </span>
      </span>
      {undo ?? <span />}
    </div>
  );
}

/** Whether a formula can go in this cell. A workspace's own cells can always hold one. */
export function holdsFormula(ws: Workspace, c: string): boolean {
  return ws.cellInfo(c)?.formulas ?? true;
}

/** Naming a new cell of the node's own. It has to read as a name, since that's what formulas call it. */
function NewCell({ ws, taken, onName, onCancel }: { ws: Workspace; taken: string[]; onName: (c: string) => void; onCancel: () => void }) {
  const [name, setName] = useState("");
  const c = name.trim();
  const problem = !c ? null : !/^[\p{L}_][\p{L}\p{N}_]*$/u.test(c) ? "A name is letters, digits and _, and starts with a letter" : ws.cellInfo(c) ? `${c} is a cell of ${label(ws.cellInfo(c)!.component).toLowerCase()}, not one of the workspace's own` : taken.includes(c) ? "It already has that" : NOT_NAMES.has(c) ? `${c} means something in a formula already` : null;
  return (
    <div className="new-cell">
      <input
        autoFocus
        placeholder="What it's called, such as price"
        aria-label="New cell's name"
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            onCancel();
          } else if (e.key === "Enter" && c && !problem) onName(c);
        }}
      />
      {problem && <div className="muted small">{problem}</div>}
      <div className="row end">
        <button onClick={onCancel}>Cancel</button>
        <button className="primary" disabled={!c || !!problem} onClick={() => onName(c)}>
          Add
        </button>
      </div>
    </div>
  );
}

/** The change an entry makes. A formula's labels go with it, so a formula replaced by a value takes them along. */
export function entryOp(target: Target, cell: string, entry: ReturnType<typeof parseEntry>, labels: Labels | undefined, wasFormula: boolean): Op {
  const on = "id" in target ? { id: target.id } : { from: target.from, to: target.to };
  const dropped = wasFormula && "id" in target ? { labels: {} } : {};
  if (entry.kind === "formula") return { op: "set", ...on, cell, formula: entry.text, ...("id" in target ? { labels } : {}) } as Op;
  return (entry.kind === "value" ? { op: "set", ...on, cell, value: entry.value, ...dropped } : { op: "set", ...on, cell, ...dropped }) as Op;
}

const NOT_NAMES = new Set(["true", "false", "undefined", "self", "from", "to", "via", "in", "out", "with", "if", "defined", "chose", "round", "floor", "ceil", "sum", "product", "all", "any", "count", "pick", "distinct", "row", "rows", "prev", "next", "abs", "min", "max", "or", "effort", "has", "whole", "is_leaf", "done_beneath", "days"]);

/** The names a formula uses that have to come from somewhere: its labels. */
function namesIn(formula: string): string[] {
  const text = formula.replace(/"(\\.|[^"\\])*"/g, '""');
  const bound = new Set([...text.matchAll(/([\p{L}_][\p{L}\p{N}_]*)\s+in\b/gu)].map((m) => m[1]));
  const names = [...text.matchAll(/(?<![\p{L}\p{N}_.])([\p{L}_][\p{L}\p{N}_]*)/gu)].map((m) => m[1]);
  return [...new Set(names)].filter((n) => !NOT_NAMES.has(n) && !bound.has(n));
}

/**
 * A cell as in a spreadsheet: one field, where `=` starts a formula and anything else is a value. A
 * formula on a node also says what its names mean. Hovering over a bracketed part says what that part
 * comes to.
 */
function CellEditor({ ws, target, c, cell, onSelect, opening, onClosed }: { ws: Workspace; target: Target; c: string; cell: Cell; onSelect: (id: string) => void; opening?: boolean; onClosed?: () => void }) {
  const formula = cell.formula;
  const id = "id" in target ? target.id : null;
  // A formula taken from a kind has its names labelled there, and typing over it starts from those.
  const labelledOn = id && cell.source === "kind" && cell.kind ? cell.kind : id;
  const existing: Labels = labelledOn ? labelsOf(ws.view!, labelledOn) : {};
  const [draft, setOpenDraft] = useState<{ text: string; labels: Labels } | null>(opening ? { text: entryText(cell), labels: existing } : null);
  const setDraft = (d: { text: string; labels: Labels } | null) => {
    setOpenDraft(d);
    if (!d) onClosed?.();
  };
  const [part, setPart] = useState<{ text: string; reads: string; error?: boolean } | null>(null);
  const [expansion, setExpansion] = useState<string | null>(null);
  const [refused, setRefused] = useState<string | null>(null);
  const asked = useRef(0);
  const name = (n: string) => ws.byId.get(n)?.name ?? n;
  const edit = () => setDraft({ text: entryText(cell), labels: existing });

  const hold = (text: string | null) => {
    const n = ++asked.current;
    if (!text || !id) return setPart(null);
    setTimeout(() => {
      if (n !== asked.current) return;
      api.tryFormula(id, text, ws.world).then((t) => {
        if (n === asked.current) setPart(t.error ? { text, reads: t.error, error: true } : { text, reads: t.shown ? shownText(t.shown, name) : "" });
      }).catch(() => {});
    }, 150);
  };

  if (draft) {
    const entry = parseEntry(draft.text);
    const uses = entry.kind === "formula" && id ? namesIn(entry.text) : [];
    const unknown = uses.filter((n) => !draft.labels[n]?.length);
    const save = async () => {
      const labels = Object.fromEntries(uses.filter((n) => draft.labels[n]?.length).map((n) => [n, draft.labels[n]]));
      try {
        await ws.apply([entryOp(target, c, entry, labels, formula !== undefined)]);
        setRefused(null);
        setDraft(null);
      } catch (e) {
        setRefused(e instanceof Error ? e.message : String(e));
      }
    };
    return (
      <div className="formula-edit">
        <AutoText
          autoFocus
          className="formula-input"
          aria-label={`${label(c)}: a value, or = and a formula`}
          value={draft.text}
          onFocus={(e) => e.currentTarget.setSelectionRange(e.currentTarget.value.length, e.currentTarget.value.length)}
          onChange={(e) => setDraft({ ...draft, text: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.stopPropagation();
              setRefused(null);
              setDraft(null);
            } else if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              save();
            }
          }}
        />
        <div className="muted small">A number or text, or start with = for a formula, such as =price * 2.</div>
        {uses.map((n) => (
          <div key={n} className="formula-label">
            <code>{n}</code>
            <span className="chips">
              {(draft.labels[n] ?? []).map((to) => (
                <span key={to} className="edge-chip">
                  <NodeChip ws={ws} id={to} onSelect={onSelect} />
                  <RemoveButton title="Take this out" onClick={() => setDraft({ ...draft, labels: { ...draft.labels, [n]: draft.labels[n].filter((x) => x !== to) } })} />
                </span>
              ))}
              <input
                list="formula-nodes"
                aria-label={`What ${n} refers to`}
                placeholder={draft.labels[n]?.length ? "and another…" : "which node?"}
                onChange={(e) => {
                  const found = ws.view!.nodes.find((x) => x.name === e.target.value && x.id !== id);
                  if (found) {
                    setDraft({ ...draft, labels: { ...draft.labels, [n]: [...(draft.labels[n] ?? []), found.id] } });
                    e.target.value = "";
                  }
                }}
              />
            </span>
          </div>
        ))}
        <datalist id="formula-nodes">
          {ws.view!.nodes.filter((x) => x.id !== id).map((x) => (
            <option key={x.id} value={x.name} />
          ))}
        </datalist>
        {refused && <div className="error">{refused}</div>}
        <div className="row end">
          <button
            onClick={() => {
              setRefused(null);
              setDraft(null);
            }}
          >
            Cancel
          </button>
          <button className="primary" disabled={unknown.length > 0} title={unknown.length ? `Say what ${unknown.join(", ")} refers to` : "⏎"} onClick={save}>
            Save
          </button>
        </div>
      </div>
    );
  }

  if (formula === undefined) {
    return (
      <div className="formula-read">
        <span className="cell-line">
          <span className="mono-value cell-plain" title="Click to edit" onClick={edit}>
            {entryText(cell) || <span className="muted">nothing yet</span>}
          </span>
          <button className="link small" onClick={edit}>
            edit
          </button>
        </span>
      </div>
    );
  }

  return (
    <div className="formula-read">
      <div className={`formula-box ${cell.source === "own" ? "" : "inherited"}`} title="Click to edit" onClick={edit}>
        <code className="formula-text">
          <span className="formula-eq">=</span>
          <Brackets text={formula} onHeld={hold} renderText={(t) => <FormulaRefs text={t} labels={existing} ws={ws} onSelect={onSelect} />} />
        </code>
        <button
          className="link small formula-edit-link"
          onClick={(e) => {
            e.stopPropagation();
            edit();
          }}
        >
          edit
        </button>
      </div>
      {part && (
        <div className="formula-part small">
          <code>{part.text}</code> {part.error ? <span className="cell-error">can't be worked out alone: {part.reads}</span> : <><span className="muted">comes to</span> {part.reads}</>}
        </div>
      )}
      {/* What a formula comes to, unless that's only the formula again, as with =undefined. */}
      {!(cell.shown && !cell.error && shownText(cell.shown, name) === formula.trim()) && (
        <div className="formula-result">
          <Resolved ws={ws} cell={cell} component={c} />
        </div>
      )}
      {id && (
        <button
          className="link small formula-expand-link"
          onClick={async () => {
            if (expansion !== null) return setExpansion(null);
            const t = await api.tryFormula(id, formula, ws.world);
            setExpansion(t.expansion ?? t.error ?? "");
          }}
        >
          {expansion === null ? "what it expands to" : "hide what it expands to"}
        </button>
      )}
      {expansion !== null && (
        <code className="formula-text formula-expansion small">
          <Brackets text={expansion} />
        </code>
      )}
    </div>
  );
}

/** A formula's text with the names it has labels for as links to what they mean. */
function FormulaRefs({ text, labels, ws, onSelect }: { text: string; labels: Labels; ws: Workspace; onSelect: (id: string) => void }) {
  const parts: React.ReactNode[] = [];
  let at = 0;
  for (const m of text.matchAll(/"(?:\\.|[^"\\])*"|[\p{L}_][\p{L}\p{N}_]*/gu)) {
    const nodes = labels[m[0]];
    if (!nodes?.length || text[m.index - 1] === ".") continue;
    parts.push(text.slice(at, m.index));
    parts.push(
      <button
        key={m.index}
        className="formula-ref"
        title={nodes.map((n) => ws.byId.get(n)?.name ?? n).join(", ")}
        onClick={(e) => {
          e.stopPropagation();
          onSelect(nodes[0]);
        }}
      >
        {m[0]}
      </button>,
    );
    at = m.index + m[0].length;
  }
  parts.push(text.slice(at));
  return <>{parts}</>;
}

/**
 * A node's edges, grouped by what they are. What each holds, such as an option's weight, shows beside it,
 * and clicking that opens its cells, so it reads the same from either end.
 */
function EdgeList({ title, id, edges, ws, onSelect, run }: { title: string; id: string; edges: Edge[]; ws: Workspace; onSelect: (id: string) => void; run: (ops: Op[], why?: string) => Promise<boolean> }) {
  const [open, setOpen] = useState<string | null>(null);
  if (!edges.length) return null;
  const other = (e: Edge) => (e.from === id ? e.to : e.from);
  const groups = new Map<string, Edge[]>();
  for (const e of edges) {
    const key = e.components.join(", ");
    groups.set(key, [...(groups.get(key) ?? []), e]);
  }
  return (
    <section>
      <h4>{title}</h4>
      {[...groups].map(([key, es]) => (
        <div key={key} className="edge-group">
          <div className="rel" title={es[0].components.map((c) => ws.info(c)?.about ?? c).join(" ")}>
            {es[0].components.map(label).join(", ")}
          </div>
          <div className="link-rows">
            {es.map((e) => {
              const holds = edgeText(e);
              const called = e.cells.called?.value;
              const cells = Object.keys(e.cells).filter((c) => !ws.cellInfo(c)?.internal);
              return (
                <div key={e.id} className="link-row">
                  <span className="edge-chip">
                    {typeof called === "string" && <code className="small">{called}</code>}
                    <NodeChip ws={ws} id={other(e)} onSelect={onSelect} />
                    {cells.length > 0 && (
                      <button className={`link-holds ${open === e.id ? "on" : ""}`} title="What this edge holds. Click to see or change it." onClick={() => setOpen(open === e.id ? null : e.id)}>
                        {holds || "…"}
                      </button>
                    )}
                    <RemoveButton title="Take this edge away" onClick={() => run(e.components.map((c) => ({ op: "unlink", from: e.from, to: e.to, component: c }) as Op))} />
                  </span>
                  {open === e.id && (
                    <div className="link-holds-edit">
                      {cells.map((c) => (
                        <CellRow key={c} ws={ws} target={{ from: e.from, to: e.to }} c={c} cell={e.cells[c]} onSelect={onSelect} />
                      ))}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      ))}
    </section>
  );
}

function LinkForm({ ws, id, onDone, run }: { ws: Workspace; id: string; onDone: () => void; run: (ops: Op[]) => Promise<boolean> }) {
  const kinds = ws.view!.components.filter((c) => c.place === "edge");
  const [kind, setKind] = useState(kinds[0]?.name ?? "");
  const [called, setCalled] = useState("");
  const [target, setTarget] = useState("");
  const [outgoing, setOutgoing] = useState(true);
  const nodes = ws.view!.nodes.filter((n) => n.id !== id);
  const other = nodes.find((n) => n.name === target.trim() || n.id === target.trim())?.id;
  return (
    <form
      className="new-node"
      onSubmit={async (e) => {
        e.preventDefault();
        if (!other) return;
        const cells: Record<string, Written> = kind === "label" ? { called: called.trim() } : {};
        if (await run([{ op: "link", from: outgoing ? id : other, to: outgoing ? other : id, component: kind, cells }])) onDone();
      }}
    >
      <div className="row small">
        <select value={outgoing ? "out" : "in"} onChange={(e) => setOutgoing(e.target.value === "out")}>
          <option value="out">this →</option>
          <option value="in">other →</option>
        </select>
        <select value={kind} onChange={(e) => setKind(e.target.value)}>
          {kinds.map((r) => (
            <option key={r.name} value={r.name} title={r.about}>
              {label(r.name)}
            </option>
          ))}
        </select>
        <span className="muted">{outgoing ? "→ other" : "→ this"}</span>
      </div>
      <input autoFocus list="node-options" placeholder="The other node" value={target} onChange={(e) => setTarget(e.target.value)} />
      {kind === "label" && <input placeholder="What a formula calls it" aria-label="What a formula calls it" value={called} onChange={(e) => setCalled(e.target.value)} />}
      <datalist id="node-options">
        {nodes.map((n) => (
          <option key={n.id} value={n.name} />
        ))}
      </datalist>
      <div className="row end">
        <button type="button" onClick={onDone}>
          Cancel
        </button>
        <button className="primary" disabled={!other}>
          Link
        </button>
      </div>
    </form>
  );
}

/** Who did what to this node, and why. */
function NodeHistory({ ws, id, onSelect }: { ws: Workspace; id: string; onSelect: (id: string) => void }) {
  const [events, setEvents] = useState<EventView[]>([]);
  const seq = ws.view!.last_seq;
  useEffect(() => {
    let live = true;
    api.history(id).then((h) => live && setEvents(h)).catch(() => {});
    return () => {
      live = false;
    };
  }, [id, seq]);
  if (!events.length) return null;
  return (
    <section>
      <h4>History</h4>
      <ul className="node-history">
        {events.slice(0, 6).map((ev) => (
          <li key={ev.seq}>
            <span className="muted small">
              {ev.author} · {new Date(ev.at).toLocaleString()}
            </span>
            {ev.why && <div className="small">{ev.why}</div>}
            <ul className="small muted">
              {ev.ops
                .filter((o) => o.node === id || o.other === id)
                .slice(0, 4)
                .map((o, i) => (
                  <li key={i}>
                    <OpLine op={o} ws={ws} onSelect={onSelect} self={id} />
                  </li>
                ))}
            </ul>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** A selected edge: its ends, and its components with their cells, as a node's are. */
export function EdgeInspector({ ws, edge: id, onSelect, onClose }: { ws: Workspace; edge: string; onSelect: (id: string) => void; onClose: () => void }) {
  const [err, setErr] = useState<string | null>(null);
  const edge = ws.edgeById.get(id);
  const run = async (ops: Op[]) => {
    try {
      setErr(null);
      await ws.apply(ops);
      return true;
    } catch (x) {
      setErr(x instanceof Error ? x.message : String(x));
      return false;
    }
  };
  return (
    <aside className="inspector">
      <div className="insp-head" style={{ borderColor: "var(--muted)" }}>
        <div className="insp-kind">
          {edge?.components.map((c) => (
            <CompBadge key={c} ws={ws} c={c} />
          ))}
          <span className="muted">edge</span>
        </div>
        <button className="icon" onClick={onClose} title="Close">
          ×
        </button>
      </div>
      {!edge ? (
        <p className="muted">This edge no longer exists.</p>
      ) : (
        <>
          <div className="edge-ends">
            <NodeChip ws={ws} id={edge.from} onSelect={onSelect} />
            <span className="edge-arrow">→</span>
            <NodeChip ws={ws} id={edge.to} onSelect={onSelect} />
          </div>
          {edge.components.map((c) => (
            <EdgeCard key={c} ws={ws} edge={edge} c={c} run={run} onSelect={onSelect} />
          ))}
          <Dropdown label={<>＋ Add</>}>
            {addable(ws, "edge", edge.components).map((c) => (
              <DdItemButton key={c.name} color={compColor(c.name)} onClick={() => run([{ op: "link", from: edge.from, to: edge.to, component: c.name }])}>
                <span className="dd-glyph">{c.icon ?? "·"}</span>
                <span>
                  <b>{label(c.name)}</b>
                  <span className="dd-desc">{c.about}</span>
                </span>
              </DdItemButton>
            ))}
          </Dropdown>
          {err && <div className="error">{err}</div>}
          <section className="danger-zone">
            <button className="danger" onClick={() => run(edge.components.map((c) => ({ op: "unlink", from: edge.from, to: edge.to, component: c }) as Op)).then((ok) => ok && onClose())}>
              Take this edge away
            </button>
          </section>
        </>
      )}
    </aside>
  );
}

function EdgeCard({ ws, edge, c, run, onSelect }: { ws: Workspace; edge: Edge; c: string; run: (ops: Op[]) => Promise<boolean>; onSelect: (id: string) => void }) {
  const info = ws.info(c);
  const cells = (info?.cells ?? []).filter((x) => edge.cells[x] && !ws.cellInfo(x)?.internal);
  return (
    <div className="comp" style={{ ["--c" as string]: compColor(c) }}>
      <div className="comp-head">
        <span className="comp-name" title={info?.about}>
          {info?.icon && <span className="glyph">{info.icon}</span>} {label(c)}
        </span>
        <RemoveButton title={`Take ${label(c).toLowerCase()} away`} onClick={() => run([{ op: "unlink", from: edge.from, to: edge.to, component: c }])} />
      </div>
      <div className="comp-body">
        {info?.about && <span className="muted small">{info.about}</span>}
        {cells.map((x) => (
          <CellRow key={x} ws={ws} target={{ from: edge.from, to: edge.to }} c={x} cell={edge.cells[x]} onSelect={onSelect} />
        ))}
      </div>
    </div>
  );
}

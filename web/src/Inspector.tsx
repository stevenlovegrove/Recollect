import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api, type Cell, type ComponentInfo, type EventView, type GraphNode, type Labels, type Link, type Op } from "./api";
import { linksOf, type Workspace } from "./workspace";
import { GLYPH, compColor, factsOf, isDropped, label, nodeColor } from "./theme";
import { DdItemButton, Dropdown } from "./Dropdown";
import { Brackets, CellValue, CompBadge, NodeChip, Resolved, shownText } from "./bits";
import { Key } from "./Shortcuts";
import { OpLine } from "./views/HistoryView";
import { askSure, askWhy } from "./Ask";

/** A node about to be made: nothing exists until it's saved. */
export type NodeTemplate = { heading: string; parent?: string; n: number };

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

export function NewNodePanel({ ws, template, onCreated, onClose }: { ws: Workspace; template: NodeTemplate; onCreated: (id: string) => void; onClose: () => void }) {
  const [name, setName] = useState("");
  const [body, setBody] = useState("");
  const [facts, setFacts] = useState<string[]>(() => (template.parent && ws.view!.workspace.systems.includes("plan") ? ["task"] : []));
  const [err, setErr] = useState<string | null>(null);
  const available = ws.view!.components.filter((c) => c.on === "node" && !c.derived && !["name", "body", "sources", "done", "dropped"].includes(c.name));
  const link = ws.partKind;

  const save = async (again: boolean) => {
    const components: Record<string, unknown> = {};
    for (const f of facts) components[f] = ws.info(f)?.initial ?? null;
    const op: Op = { op: "add_node", id: "new:node", name: name.trim() || "Untitled", components, links: template.parent && link ? [{ to: template.parent, kind: link, value: ws.info(link)?.initial ?? null }] : [] };
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
              <input type="checkbox" checked={facts.includes(c.name)} onChange={() => setFacts(facts.includes(c.name) ? facts.filter((f) => f !== c.name) : [...facts, c.name])} />
              {GLYPH[c.name] ? `${GLYPH[c.name]} ` : ""}
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

export function Inspector({
  ws,
  id,
  onSelect,
  onClose,
  onAdd,
  command,
}: {
  ws: Workspace;
  id: string;
  onSelect: (id: string) => void;
  onClose: () => void;
  onAdd: (t: Omit<NodeTemplate, "n">) => void;
  /** Keyboard requests from the app; `n` changes each time one is made. */
  command?: { kind: "edit" | "link"; n: number } | null;
}) {
  const node = ws.byId.get(id)!;
  const view = ws.view!;
  const [err, setErr] = useState<string | null>(null);
  const [linking, setLinking] = useState(false);
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

  const text = (c: string) => (typeof node.cells[c]?.value === "string" ? (node.cells[c].value as string) : "");
  const name = useDraft(text("name"), (v) => v.trim() && run([{ op: "set", id, component: "name", value: v.trim() }]));
  const notes = useDraft(text("body"), (v) => run(v.trim() ? [{ op: "set", id, component: "body", value: v.trimEnd() }] : [{ op: "set", id, component: "body" }]));

  const { outgoing, incoming } = linksOf(view, id);
  const facts = factsOf(node);
  const stored = view.components.filter((c) => c.on === "node" && !["name", "body"].includes(c.name) && node.cells[c.name]?.source === "stored");
  const computedHere = Object.entries(node.cells).filter(([c, cell]) => cell.source === "rule" && !ws.info(c)?.quiet);
  const missing = view.components.filter((c) => c.on === "node" && !c.derived && !node.cells[c.name]);
  const others = Object.entries(node.cells).filter(([c, cell]) => cell.source === "stored" && !view.components.some((x) => x.name === c) && c !== "name" && c !== "body");

  return (
    <aside className="inspector">
      <div className="insp-head" style={{ borderColor: nodeColor(node) }}>
        <div className="insp-kind">{facts.length ? facts.map((c) => <CompBadge key={c} c={c} />) : <span className="muted">note</span>}</div>
        <button className="icon" onClick={onClose} title="Close (esc)">
          ×
        </button>
      </div>

      <div className="insp-form">
        <div className="key-host">
          <Key k="e" />
          <AutoText inputRef={nameRef} className="insp-title-input" aria-label="Name" placeholder="Name" {...name} onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()} />
        </div>
        <AutoText className="insp-notes" aria-label="Notes" placeholder="Notes…" {...notes} />
        <code className="muted small">{id}</code>
      </div>

      {err && <div className="error">{err}</div>}

      <section>
        <h4>What it is</h4>
        {stored.length === 0 && others.length === 0 && !computedHere.some(([, cell]) => cell.own) && <p className="muted small">A plain note. Add something to give it behaviour, or leave it as a note.</p>}
        {stored.map((c) => (
          <FactEditor key={c.name} node={node} info={c} ws={ws} run={run} />
        ))}
        {others.map(([c, cell]) => (
          <div key={c} className="comp">
            <div className="comp-head">
              <span className="comp-name" title="No active system says what this means. It's kept as it is.">
                {c}
              </span>
              {(
                <button className="icon small" title={`Remove ${c}`} onClick={() => run([{ op: "set", id, component: c }])}>
                  ×
                </button>
              )}
            </div>
            <div className="comp-body">
              <CellValue ws={ws} cell={cell} onSelect={onSelect} />
            </div>
          </div>
        ))}
        {missing.length > 0  && (
          <Dropdown label={<>＋ Add</>}>
            {missing.map((c) => (
              <DdItemButton key={c.name} color={compColor(c.name)} onClick={() => run([{ op: "set", id, component: c.name, value: c.initial ?? null }])}>
                <span className="dd-glyph">{GLYPH[c.name] ?? "·"}</span>
                <span>
                  <b>{label(c.name)}</b>
                  <span className="dd-desc">{c.about}</span>
                </span>
              </DdItemButton>
            ))}
          </Dropdown>
        )}
      </section>

      {computedHere.length > 0 && (
        <section>
          <h4>Worked out</h4>
          {computedHere.map(([c, cell]) => (
            <Derived key={c} ws={ws} id={id} c={c} cell={cell} info={ws.info(c)} onSelect={onSelect} run={run} />
          ))}
        </section>
      )}

      <LinkList title="Goes to" links={outgoing} side="to" ws={ws} onSelect={onSelect} run={run} />
      <LinkList title="Comes from" links={incoming} side="from" ws={ws} onSelect={onSelect} run={run} />

      {(
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
      )}
      {linking && <LinkForm ws={ws} id={id} onDone={() => setLinking(false)} run={run} />}

      <NodeHistory ws={ws} id={id} onSelect={onSelect} />

      {(
        <section className="danger-zone">
          {ws.info("dropped") && (
            <button
              className="key-host"
              onClick={async () => {
                if (isDropped(node)) run([{ op: "set", id, component: "dropped" }], "brought back");
                else {
                  const why = await askWhy(`Drop “${node.name}”?`, "Drop");
                  if (why !== null) run([{ op: "set", id, component: "dropped", value: null }], why);
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
              if (await askSure(`Delete “${node.name}” and its links? It stays in the log, and can be undone from there.`, "Delete")) run([{ op: "delete_node", id }], "deleted");
            }}
          >
            Delete
          </button>
        </section>
      )}
    </aside>
  );
}

/** Edit a stored fact: as a number, as text, or as fields, whichever it is. */
function FactEditor({ node, info, ws, run }: { node: GraphNode; info: ComponentInfo; ws: Workspace; run: (ops: Op[], why?: string) => Promise<boolean> }) {
  const id = node.id;
  const c = info.name;
  const cell = node.cells[c];
  const value = cell.value;
  const set = (v: unknown, why = "") => run([{ op: "set", id, component: c, value: v }], why);
  const rules = ws.view!.rules.filter((r) => r.output === c && !r.key.startsWith("expr@"));
  const formulaCan = ws.view!.rules.some((r) => r.key.startsWith("expr@") && (r.output === c || r.also?.includes(c)));
  return (
    <div className="comp" style={{ ["--c" as string]: compColor(c) }}>
      <div className="comp-head">
        <span className="comp-name" title={info.about}>
          {GLYPH[c] && <span className="glyph">{GLYPH[c]}</span>} {label(c)}
        </span>
        {(
          <button className="icon small" title={`Remove ${label(c).toLowerCase()}`} onClick={() => run([{ op: "set", id, component: c }])}>
            ×
          </button>
        )}
      </div>
      {c === "decision" ? (
        <div className="comp-body small">
          {(value as { chosen?: string }).chosen ? (
            <>
              Chose <NodeChip ws={ws} id={(value as { chosen: string }).chosen} onSelect={() => {}} />
            </>
          ) : (
            <span className="muted">Still open. Its options are on the Decisions tab.</span>
          )}
        </div>
      ) : value !== null && value !== undefined && (
        <div className="comp-body">
          <ValueEditor key={JSON.stringify(value)} value={value} onCommit={set} />
          {(rules.length > 0 || formulaCan) && (
            <div className="muted small">
              or work it out with{" "}
              {formulaCan && (
                <button className="link" onClick={() => run([{ op: "set", id, component: c, formula: typeof value === "string" ? JSON.stringify(value) : String(value) }])}>
                  a formula
                </button>
              )}
              {rules.map((r) => (
                <button key={r.key} className="link" onClick={() => run([{ op: "set", id, component: c, rule: r.key }])}>
                  {r.key}
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function ValueEditor({ value, onCommit }: { value: unknown; onCommit: (v: unknown) => void }) {
  const [text, setText] = useState(typeof value === "string" ? value : JSON.stringify(value));
  const [bad, setBad] = useState(false);
  if (typeof value === "number") {
    return (
      <input
        type="number"
        step="any"
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => text !== "" && Number(text) !== value && onCommit(Number(text))}
        onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
      />
    );
  }
  if (typeof value === "string" && value.includes("\n")) {
    return <textarea className="value-text" rows={Math.min(24, value.split("\n").length + 1)} spellCheck={false} value={text} onChange={(e) => setText(e.target.value)} onBlur={() => text !== value && onCommit(text)} />;
  }
  if (typeof value === "string") {
    return <input value={text} onChange={(e) => setText(e.target.value)} onBlur={() => text !== value && onCommit(text)} onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()} />;
  }
  // Fields of a plain object get their own inputs, and anything else is edited as it's written.
  if (value && typeof value === "object" && !Array.isArray(value) && Object.values(value).every((v) => typeof v === "number" || typeof v === "string")) {
    const fields = value as Record<string, unknown>;
    return (
      <div className="fields">
        {Object.entries(fields).map(([k, v]) => (
          <label key={k} className="labelled">
            <span className="labelled-l">{label(k)}</span>
            <span className="labelled-c">
              <ValueEditor value={v} onCommit={(next) => onCommit({ ...fields, [k]: next })} />
            </span>
          </label>
        ))}
      </div>
    );
  }
  return (
    <textarea
      className={bad ? "bad" : undefined}
      rows={2}
      value={text}
      onChange={(e) => {
        setText(e.target.value);
        setBad(false);
      }}
      onBlur={() => {
        try {
          const next = JSON.parse(text);
          if (JSON.stringify(next) !== JSON.stringify(value)) onCommit(next);
        } catch {
          setBad(true);
        }
      }}
    />
  );
}

const NOT_NAMES = new Set(["true", "false", "undefined", "self", "in", "out", "with", "if", "defined", "chose", "round", "floor", "ceil", "sum", "product", "all", "any", "count", "pick", "abs", "min", "max", "or", "effort"]);

/** The names a formula uses that have to come from somewhere: its labels. */
function namesIn(formula: string): string[] {
  const text = formula.replace(/"(\\.|[^"\\])*"/g, '""');
  const bound = new Set([...text.matchAll(/([\p{L}_][\p{L}\p{N}_]*)\s+in\b/gu)].map((m) => m[1]));
  const names = [...text.matchAll(/(?<![\p{L}\p{N}_.])([\p{L}_][\p{L}\p{N}_]*)/gu)].map((m) => m[1]);
  return [...new Set(names)].filter((n) => !NOT_NAMES.has(n) && !bound.has(n));
}

/** A formula, and what its names mean. Hovering over a bracketed part says what that part comes to. */
function FormulaEditor({ ws, id, c, cell, onSelect }: { ws: Workspace; id: string; c: string; cell: Cell; onSelect: (id: string) => void }) {
  const formula = cell.formula!;
  const existing: Labels = {};
  for (const l of ws.view!.links) if (l.from === id && l.kind === "label") (existing[String(l.value)] ??= []).push(l.to);
  const [draft, setDraft] = useState<{ text: string; labels: Labels } | null>(null);
  const [part, setPart] = useState<{ text: string; reads: string } | null>(null);
  const [expansion, setExpansion] = useState<string | null>(null);
  const [refused, setRefused] = useState<string | null>(null);
  const asked = useRef(0);
  const name = (n: string) => ws.byId.get(n)?.name ?? n;

  const hold = (text: string | null) => {
    const n = ++asked.current;
    if (!text) return setPart(null);
    setTimeout(() => {
      if (n !== asked.current) return;
      api.tryFormula(id, text, ws.world).then((t) => {
        if (n === asked.current) setPart({ text, reads: t.error ?? (t.shown ? shownText(t.shown, name) : "") });
      }).catch(() => {});
    }, 150);
  };

  if (draft) {
    const uses = namesIn(draft.text);
    const unknown = uses.filter((n) => !draft.labels[n]?.length);
    const save = async () => {
      const labels = Object.fromEntries(uses.filter((n) => draft.labels[n]?.length).map((n) => [n, draft.labels[n]]));
      try {
        await ws.apply([{ op: "set", id, component: c, formula: draft.text.trim(), labels }]);
        setRefused(null);
        setDraft(null);
      } catch (e) {
        setRefused(e instanceof Error ? e.message : String(e));
      }
    };
    return (
      <div className="formula-edit">
        <textarea
          autoFocus
          className="formula-input"
          rows={2}
          aria-label="Formula"
          value={draft.text}
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
        {uses.map((n) => (
          <div key={n} className="formula-label">
            <code>{n}</code>
            <span className="chips">
              {(draft.labels[n] ?? []).map((to) => (
                <span key={to} className="edge-chip">
                  <NodeChip ws={ws} id={to} onSelect={onSelect} />
                  <button className="icon small" title="Take this out" onClick={() => setDraft({ ...draft, labels: { ...draft.labels, [n]: draft.labels[n].filter((x) => x !== to) } })}>
                    ×
                  </button>
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
          <button className="primary" disabled={unknown.length > 0 || !draft.text.trim()} title={unknown.length ? `Say what ${unknown.join(", ")} refers to` : "⏎"} onClick={save}>
            Save
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="formula-read">
      <code className="formula-text" title="Click to edit" onClick={() => setDraft({ text: formula, labels: existing })}>
        <Brackets text={formula} onHeld={hold} renderText={(t) => <FormulaRefs text={t} labels={existing} ws={ws} onSelect={onSelect} />} />
      </code>
      <Resolved ws={ws} cell={cell} component={c} />
      {part && (
        <div className="formula-part small">
          <code>{part.text}</code> <span className="muted">is</span> {part.reads}
        </div>
      )}
      <button
        className="link small"
        onClick={async () => {
          if (expansion !== null) return setExpansion(null);
          const t = await api.tryFormula(id, formula, ws.world);
          setExpansion(t.expansion ?? t.error ?? "");
        }}
      >
        {expansion === null ? "what it expands to" : "hide"}
      </button>
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

function Derived({ ws, id, c, cell, info, onSelect, run }: { ws: Workspace; id: string; c: string; cell: Cell; info: ComponentInfo | undefined; onSelect: (id: string) => void; run: (ops: Op[], why?: string) => Promise<boolean> }) {
  const canType = cell.own && info && !info.derived;
  return (
    <div className="derived">
      <span className="derived-k" title={info?.about}>
        {label(c)}
        {cell.own && cell.formula === undefined && <span className="muted small"> · {cell.rule}</span>}
      </span>
      <span className="derived-v">
        {cell.formula !== undefined && cell.own ? <FormulaEditor key={cell.formula} ws={ws} id={id} c={c} cell={cell} onSelect={onSelect} /> : <CellValue ws={ws} cell={cell} onSelect={onSelect} component={c} />}
      </span>
      {canType && (
        <button className="link small" title="Replace the formula with this number" onClick={() => run([{ op: "set", id, component: c, value: cell.value ?? info.initial ?? null }])}>
          type over
        </button>
      )}
    </div>
  );
}

function LinkList({ title, links, side, ws, onSelect, run }: { title: string; links: Link[]; side: "from" | "to"; ws: Workspace; onSelect: (id: string) => void; run: (ops: Op[], why?: string) => Promise<boolean> }) {
  if (!links.length) return null;
  const byKind = new Map<string, Link[]>();
  for (const l of links) byKind.set(l.kind, [...(byKind.get(l.kind) ?? []), l]);
  return (
    <section>
      <h4>{title}</h4>
      {[...byKind].map(([kind, ls]) => (
        <div key={kind} className="edge-group">
          <div className="rel" title={ws.info(kind)?.about}>
            {label(kind)}
          </div>
          <div className="chips">
            {ls.map((l) => (
              <span key={l[side]} className="edge-chip">
                {kind === "label" && <code className="small">{String(l.value)}</code>}
                <NodeChip ws={ws} id={l[side]} onSelect={onSelect} />
                {(
                  <button className="icon small" title="Remove this link" onClick={() => run([{ op: "link", from: l.from, to: l.to, kind: l.kind }])}>
                    ×
                  </button>
                )}
              </span>
            ))}
          </div>
        </div>
      ))}
    </section>
  );
}

function LinkForm({ ws, id, onDone, run }: { ws: Workspace; id: string; onDone: () => void; run: (ops: Op[]) => Promise<boolean> }) {
  const kinds = ws.view!.components.filter((c) => c.on === "link");
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
        const value = kind === "label" ? called.trim() : ws.info(kind)?.initial ?? null;
        if (await run([{ op: "link", from: outgoing ? id : other, to: outgoing ? other : id, kind, value }])) onDone();
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

/** A selected link. It lives on its source node, and can be read or removed here. */
export function LinkInspector({ ws, edge, onSelect, onClose }: { ws: Workspace; edge: { from: string; kind: string; to: string }; onSelect: (id: string) => void; onClose: () => void }) {
  const [err, setErr] = useState<string | null>(null);
  const link = ws.view!.links.find((l) => l.from === edge.from && l.kind === edge.kind && l.to === edge.to);
  const info = ws.info(edge.kind);
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
          <span className={`dd-swatch rel-${edge.kind}`} /> link
        </div>
        <button className="icon" onClick={onClose} title="Close">
          ×
        </button>
      </div>
      {!link ? (
        <p className="muted">This link no longer exists.</p>
      ) : (
        <>
          <h2 className="insp-title">{label(edge.kind)}</h2>
          {info && <p className="muted small">{info.about}</p>}
          <div className="edge-ends">
            <NodeChip ws={ws} id={edge.from} onSelect={onSelect} />
            <span className="edge-arrow">↓</span>
            <NodeChip ws={ws} id={edge.to} onSelect={onSelect} />
          </div>
          {link.value !== null && (
            <section>
              <h4>What it holds</h4>
              <ValueEditor key={JSON.stringify(link.value)} value={link.value} onCommit={(value) => run([{ op: "link", from: edge.from, to: edge.to, kind: edge.kind, value }])} />
            </section>
          )}
          {err && <div className="error">{err}</div>}
          {(
            <section className="danger-zone">
              <button className="danger" onClick={() => run([{ op: "link", from: edge.from, to: edge.to, kind: edge.kind }]).then((ok) => ok && onClose())}>
                Remove this link
              </button>
            </section>
          )}
        </>
      )}
    </aside>
  );
}

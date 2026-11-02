import { useEffect, useMemo, useRef, useState } from "react";
import type { ViewProps } from "./MapView";

/**
 * Where a drawing runs. A drawing is code from a workspace, which may have come from a `git pull` or a
 * proposal, and the local server has no login, so it gets nothing:
 *
 * - The frame is sandboxed with no origin of its own, so it can't read the app or call its API.
 * - Its content security policy allows no network at all, which also covers the worker it starts.
 * - The drawing runs in that worker, with no DOM, and is stopped if it takes too long, so a loop in
 *   one can't hang anything.
 * - What it returns has anything that could run or load something taken out before it's shown, and
 *   only this frame's own script, by its nonce, can run.
 *
 * All that comes back is messages, which `DrawingFrame` checks: a pin must name a real choice and option.
 */
const BOOT = `<!doctype html>
<html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'nonce-boot' 'unsafe-eval' blob:; worker-src blob:; style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'">
<style>
  html, body { margin: 0; background: var(--bg); color: var(--text); font: 13px ui-sans-serif, system-ui, sans-serif; }
  svg { max-width: 100%; height: auto; display: block; font-family: inherit; }
  text { user-select: none; }
</style></head>
<body><div id="root"></div>
<script nonce="boot">
const LONGEST = 2000;
const WORKER = \`
  onmessage = (e) => {
    try {
      const exports = {};
      new Function("exports", e.data.code)(exports);
      if (typeof exports.draw !== "function") throw new Error("A drawing has to export a function called draw.");
      postMessage({ out: String(exports.draw(e.data.world)) });
    } catch (err) {
      postMessage({ error: String(err) });
    }
  };
\`;
const root = document.getElementById("root");
const tell = (m) => parent.postMessage(m, "*");
const url = URL.createObjectURL(new Blob([WORKER], { type: "text/javascript" }));
let worker = null, code = null, timer = null;

function stop() {
  clearTimeout(timer);
  worker?.terminate();
  worker = null;
}

// A new worker every time, so a drawing can't remember anything from one world to the next, and
// nothing it started is still running once it has answered.
function run(world) {
  stop();
  if (code === null) return;
  const mine = (worker = new Worker(url));
  timer = setTimeout(() => {
    stop();
    tell({ type: "error", message: "The drawing took more than " + LONGEST / 1000 + " seconds, so it was stopped." });
  }, LONGEST);
  mine.onerror = (e) => {
    stop();
    tell({ type: "error", message: e.message || "The drawing failed." });
  };
  mine.onmessage = (e) => {
    stop();
    if (e.data.error !== undefined) return tell({ type: "error", message: e.data.error });
    show(e.data.out);
    tell({ type: "drawn" });
  };
  mine.postMessage({ code, world });
}

const NEVER = new Set(["SCRIPT", "IFRAME", "OBJECT", "EMBED", "META", "LINK", "BASE", "FORM", "FRAME", "FRAMESET", "AUDIO", "VIDEO", "SOURCE", "TRACK", "APPLET"].map((t) => t.toLowerCase()));
function clean(el) {
  for (const child of [...el.children]) {
    if (NEVER.has(child.tagName.toLowerCase())) { child.remove(); continue; }
    for (const a of [...child.attributes]) {
      const name = a.name.toLowerCase();
      const value = a.value.trim().toLowerCase();
      const loads = name === "href" || name === "xlink:href" || name === "src" || name === "srcset" || name === "action" || name === "formaction" || name === "ping" || name === "background";
      if (name.startsWith("on") || (loads && !value.startsWith("#") && !value.startsWith("data:image/"))) child.removeAttribute(a.name);
    }
    clean(child);
  }
}

function show(markup) {
  const t = document.createElement("template");
  t.innerHTML = markup;
  clean(t.content);
  root.replaceChildren(t.content);
  size();
}

function size() {
  tell({ type: "size", height: Math.ceil(document.documentElement.scrollHeight) });
}
new ResizeObserver(size).observe(document.documentElement);

document.addEventListener("click", (e) => {
  const a = e.target.closest?.("a");
  if (a) e.preventDefault();
  const el = e.target.closest?.("[data-pin-choice]");
  if (el) tell({ type: "pin", choice: el.getAttribute("data-pin-choice"), option: el.getAttribute("data-pin-option") });
});

addEventListener("message", (e) => {
  if (e.source !== parent) return;
  const m = e.data;
  if (m.theme) for (const [k, v] of Object.entries(m.theme)) document.documentElement.style.setProperty(k, v);
  if (m.code !== undefined) code = m.code;
  if (m.world !== undefined) run(m.world);
});
tell({ type: "ready" });
</script></body></html>`;

/** What a drawing is given. See docs/drawings.md. */
function worldFor(ws: ViewProps["ws"]) {
  const g = ws.view!;
  return {
    choices: g.worlds.vars,
    count: g.worlds.count,
    constrained: g.worlds.constrained,
    solutions: g.worlds.solutions,
    picks: g.worlds.picks,
    nodes: g.nodes.map((n) => {
      const { drawing: _, ...cells } = n.cells;
      return { id: n.id, name: n.name, cells };
    }),
    links: g.links,
  };
}

function themeNow() {
  const style = getComputedStyle(document.documentElement);
  return Object.fromEntries(["--text", "--muted", "--accent", "--bg", "--panel", "--border", "--ok", "--warn", "--danger"].map((k) => [k, style.getPropertyValue(k).trim()]));
}

/** A drawing, in its sandbox. If it can't be drawn, `onFailed` says why, and the app shows its own view. */
export function DrawingFrame({ ws, source, onFailed }: { ws: ViewProps["ws"]; source: string; onFailed: (why: string | null) => void }) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [ready, setReady] = useState(false);
  const [height, setHeight] = useState(240);
  const [code, setCode] = useState<string | null>(null);
  const world = useMemo(() => worldFor(ws), [ws.view]); // eslint-disable-line react-hooks/exhaustive-deps

  // Types are only stripped here, in the app, and nothing from the drawing runs.
  useEffect(() => {
    let stale = false;
    setCode(null);
    import("sucrase")
      .then(({ transform }) => {
        if (stale) return;
        setCode(transform(source, { transforms: ["typescript", "imports"], disableESTransforms: true }).code);
        onFailed(null);
      })
      .catch((e) => {
        if (!stale) onFailed(e instanceof Error ? e.message : String(e));
      });
    return () => {
      stale = true;
    };
  }, [source]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (ready && code !== null) frame.current?.contentWindow?.postMessage({ code, world, theme: themeNow() }, "*");
  }, [ready, code]); // eslint-disable-line react-hooks/exhaustive-deps
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    if (ready && code !== null) frame.current?.contentWindow?.postMessage({ world, theme: themeNow() }, "*");
  }, [world]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      if (e.source !== frame.current?.contentWindow) return;
      const m = e.data as { type?: string; height?: unknown; message?: unknown; choice?: unknown; option?: unknown };
      if (m.type === "ready") setReady(true);
      else if (m.type === "drawn") onFailed(null);
      else if (m.type === "size" && typeof m.height === "number") setHeight(Math.min(Math.max(m.height, 40), 4000));
      else if (m.type === "error") onFailed(String(m.message));
      else if (m.type === "pin") {
        const choice = ws.view!.worlds.vars.find((v) => v.id === m.choice);
        const option = choice?.options.find((o) => o.id === m.option);
        if (choice && option) ws.pin(choice.id, option.picked ? null : option.id);
      }
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  });

  return <iframe ref={frame} className="drawing-frame" title="Drawing" sandbox="allow-scripts" srcDoc={BOOT} style={{ height }} />;
}

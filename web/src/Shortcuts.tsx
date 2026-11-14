import { useEffect, useState } from "react";

/** True when a key press belongs to a text field rather than to the app. */
export function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  if (target.closest("textarea, select, [contenteditable]")) return true;
  const input = target.closest("input");
  return !!input && !["checkbox", "radio", "range", "button"].includes(input.type);
}

const HOLD_MS = 300;

/**
 * True while ⌘ is held on its own. The short delay and the cancel on any other key keep
 * this from flashing during ⌘C, ⌘Tab and friends. macOS doesn't deliver the key-up if the
 * window loses focus while ⌘ is down, so blur has to clear it too.
 */
export function useHoldMeta(): boolean {
  const [held, setHeld] = useState(false);
  useEffect(() => {
    let timer: number | undefined;
    const clear = () => {
      window.clearTimeout(timer);
      setHeld(false);
    };
    const onDown = (e: KeyboardEvent) => {
      if (e.key === "Meta" && !e.repeat) {
        window.clearTimeout(timer);
        timer = window.setTimeout(() => setHeld(true), HOLD_MS);
      } else if (e.key !== "Meta") clear();
    };
    const onUp = (e: KeyboardEvent) => {
      if (e.key === "Meta") clear();
    };
    const onHidden = () => document.hidden && clear();
    window.addEventListener("keydown", onDown, true);
    window.addEventListener("keyup", onUp, true);
    window.addEventListener("blur", clear);
    window.addEventListener("mousedown", clear, true);
    document.addEventListener("visibilitychange", onHidden);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("keydown", onDown, true);
      window.removeEventListener("keyup", onUp, true);
      window.removeEventListener("blur", clear);
      window.removeEventListener("mousedown", clear, true);
      document.removeEventListener("visibilitychange", onHidden);
    };
  }, []);
  return held;
}

/** A key badge on a control, shown only while hints are on. The parent needs `key-host`. */
export function Key({ k }: { k: string }) {
  return (
    <kbd className="key-hint" aria-hidden="true">
      {k}
    </kbd>
  );
}

type Row = [keys: string, label: string, needsSelection?: boolean];

const ANYWHERE: Row[] = [
  ["n", "New node"],
  ["w", "Every choice, and what's left"],
  ["1–9", "Switch tab"],
  ["?", "Keep this sheet open"],
  ["esc", "Close, or clear the selection"],
  ["⌘Z", "Undo"],
  ["⇧⌘Z", "Redo"],
];

const SELECTION: Row[] = [
  ["c", "New part of this", true],
  ["s", "New sibling", true],
  ["e  ⏎", "Edit the name", true],
  ["l", "Link to another node", true],
  ["x", "Toggle done", true],
  ["⌫", "Drop (asks why, can be brought back)", true],
];

const BY_TAB: Record<string, [string, Row[]]> = {
  map: [
    "Map",
    [
      ["← ↑ → ↓", "Move to the nearest card"],
      ["z", "Zoom to the selected card", true],
      ["f", "Fit everything"],
      ["0", "Zoom to 100%"],
      ["m", "Show / hide the minimap"],
      ["⇧ drag", "Draw a link from one card to another"],
      ["+  −", "Zoom in / out"],
      ["/", "Search"],
      ["⏎ ⇧⏎", "In search: next / previous match"],
      ["⌘G ⇧⌘G", "Next / previous match"],
      ["esc", "Clear the search"],
    ],
  ],
  outline: [
    "Outline",
    [
      ["↑ ↓", "Previous / next row"],
      ["←", "Collapse, then go to parent", true],
      ["→", "Expand, then first child", true],
    ],
  ],
};

export function ShortcutSheet({ tab, hasSelection, sticky, onClose }: { tab: string; hasSelection: boolean; sticky: boolean; onClose: () => void }) {
  const extra = BY_TAB[tab];
  const group = (title: string, rows: Row[]) => (
    <div className="sheet-group" key={title}>
      <h5>{title}</h5>
      {rows.map(([keys, label, needs]) => (
        <div key={keys} className={`sheet-row ${needs && !hasSelection ? "off" : ""}`}>
          <span className="sheet-keys">
            {keys.split(/\s+/).map((k) => (
              <kbd key={k}>{k}</kbd>
            ))}
          </span>
          <span>{label}</span>
        </div>
      ))}
    </div>
  );
  return (
    <div className={`shortcut-sheet ${sticky ? "sticky" : ""}`} role="dialog" aria-label="Keyboard shortcuts">
      <div className="sheet-head">
        <span>Keyboard shortcuts</span>
        {sticky ? (
          <button className="icon small" onClick={onClose} title="Close (esc)">
            ×
          </button>
        ) : (
          <span className="muted small">release ⌘, then press the key</span>
        )}
      </div>
      <div className="sheet-body">
        {group("Anywhere", ANYWHERE)}
        {group(hasSelection ? "This node" : "With a node selected", SELECTION)}
        {extra && group(extra[0], extra[1])}
      </div>
    </div>
  );
}

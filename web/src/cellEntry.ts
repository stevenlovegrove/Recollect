/**
 * What someone types into a cell, as in a spreadsheet: `=` starts a formula, and anything else is a
 * value. A leading `'` means take the rest literally, for text that really starts with `=`.
 */
export type Entry = { kind: "formula"; text: string } | { kind: "value"; value: unknown } | { kind: "empty" };

export function parseEntry(typed: string): Entry {
  const text = typed.trim();
  if (!text) return { kind: "empty" };
  if (text.startsWith("=")) {
    const formula = text.slice(1).trim();
    return formula ? { kind: "formula", text: formula } : { kind: "empty" };
  }
  if (text.startsWith("'")) return { kind: "value", value: text.slice(1) };
  if (text === "true") return { kind: "value", value: true };
  if (text === "false") return { kind: "value", value: false };
  if (Number.isFinite(Number(text))) return { kind: "value", value: Number(text) };
  if (/^[[{"]/.test(text)) {
    try {
      return { kind: "value", value: JSON.parse(text) };
    } catch {
      // It only looked like JSON, so it's text.
    }
  }
  return { kind: "value", value: text };
}

/** What a cell shows when it's being edited: what you'd type to get what it holds. */
export function entryText(cell: { formula?: string; value?: unknown; shown?: { kind: string; always?: boolean; never?: boolean } }): string {
  if (cell.formula !== undefined) return `=${cell.formula}`;
  const v = cell.value ?? (cell.shown?.always ? true : cell.shown?.never ? false : undefined);
  if (v === undefined || v === null) return "";
  if (typeof v === "string") return v.startsWith("=") || v.startsWith("'") || parseEntry(v).kind !== "value" || (parseEntry(v) as { value: unknown }).value !== v ? `'${v}` : v;
  return typeof v === "object" ? JSON.stringify(v) : String(v);
}

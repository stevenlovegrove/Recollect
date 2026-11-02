import { marked } from "marked";

/** Render notes. Raw HTML is escaped, because they may be written by an LLM. */
export function renderMarkdown(body: string): string {
  const escaped = body.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  return marked.parse(escaped, { async: false, gfm: true, breaks: false }) as string;
}

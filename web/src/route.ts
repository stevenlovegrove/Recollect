import { useCallback, useEffect, useRef, useState } from "react";
import type { Workspace } from "./workspace";

/**
 * Which workspace the address says, so a link to an example can be shared and one of your own
 * bookmarked: `?example=monty-hall` for an example, `?w=<id>` for your own, with the page after `#` as
 * before. Your own is only somewhere this browser (or this server) has it, so its link works there.
 */
export type Target = { example: string } | { own: string };

export function targetOfUrl(): Target | null {
  const q = new URLSearchParams(location.search);
  const example = q.get("example");
  if (example) return { example };
  const own = q.get("w");
  return own ? { own } : null;
}

function urlOf(t: Target | null): string {
  const q = new URLSearchParams(location.search);
  q.delete("example");
  q.delete("w");
  if (t && "example" in t) q.set("example", t.example);
  else if (t) q.set("w", t.own);
  const search = q.toString();
  return `${location.pathname}${search ? `?${search}` : ""}${location.hash}`;
}

const same = (a: Target | null, b: Target | null) => JSON.stringify(a) === JSON.stringify(b);

/** What a link names that couldn't just be opened: one that needs a click to allow, or isn't here. */
export type Waiting = { target: Target; name?: string; why: "allow" | "missing"; message?: string };

/**
 * Keeps the address and the open workspace in step. On arriving, what the address names is opened in
 * place of whatever was open last. Opening another puts it in the address, so Back goes back to the one
 * before; and Back or Forward to an address opens what it names.
 */
export function useRoute(ws: Workspace) {
  const current = ws.available?.current;
  const here: Target | null = !current ? null : current.sample ? { example: current.sample } : { own: current.id };
  const [waiting, setWaiting] = useState<Waiting | null>(null);
  // Until what the address named on arriving has been opened, or couldn't be, the address isn't touched.
  const settled = useRef(false);
  // What the address asked for and is being opened, by arriving or by Back and Forward. Until it's
  // open, what's still showing is on its way out, and isn't put back in the address.
  const opening = useRef<Target | null>(null);

  const open = useCallback(
    async (t: Target) => {
      const b = ws.backend;
      if (!b) return;
      setWaiting(null);
      opening.current = t;
      try {
        if ("example" in t) await ws.openSample(t.example);
        else if (b.kind === "http") await b.openPath!(t.own);
        else await b.openKnown!(t.own);
      } catch (e) {
        opening.current = null;
        const message = e instanceof Error ? e.message : String(e);
        const known = ws.available?.recent?.find((k) => "own" in t && k.id === t.own);
        // A folder has to be allowed again from a click, which arriving at a link isn't.
        setWaiting({ target: t, name: known?.name, why: known ? "allow" : "missing", message });
      }
    },
    [ws],
  );

  // Arriving: open what the address names, once whatever was open last has had its chance to open.
  useEffect(() => {
    if (settled.current || !ws.backend || !ws.available) return;
    settled.current = true;
    const wanted = targetOfUrl();
    if (wanted && !same(wanted, here)) open(wanted);
  }, [ws.backend, ws.available, here, open]);

  // Opening another: say so in the address. It's a new entry, so Back goes back to the one before.
  const shown = useRef<Target | null>(null);
  const nothingOpen = ws.nothingOpen;
  useEffect(() => {
    if (!settled.current || waiting) return;
    // Nothing open, as after deleting what was: the address names nothing either.
    if (!here) {
      if (nothingOpen && !opening.current && targetOfUrl()) {
        shown.current = null;
        history.replaceState(history.state, "", urlOf(null));
      }
      return;
    }
    if (opening.current) {
      if (!same(here, opening.current)) return;
      opening.current = null;
      shown.current = here;
      return;
    }
    if (same(here, shown.current) && same(here, targetOfUrl())) return;
    const first = shown.current === null;
    shown.current = here;
    if (same(here, targetOfUrl())) return;
    if (first) history.replaceState(history.state, "", urlOf(here));
    else history.pushState(history.state, "", urlOf(here));
  }, [here, waiting, nothingOpen]);

  // Back and Forward: open what the address names now.
  useEffect(() => {
    const onPop = () => {
      const wanted = targetOfUrl();
      if (wanted && !same(wanted, shown.current)) open(wanted);
    };
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, [open]);

  return { waiting, retry: () => waiting && open(waiting.target), dismiss: () => setWaiting(null) };
}

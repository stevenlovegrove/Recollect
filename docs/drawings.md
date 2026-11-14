# Drawings

A drawing shows a problem the way it's really thought about: a sudoku with its boxes, a colouring as a map. It's a node with a `drawing` component, whose `drawing` cell holds TypeScript, so it's in the log, with who wrote it and why, and can be undone like anything else, and a workspace can have several. The worlds panel, which the worlds chip on every page opens into, has a **Show** menu to pick one or the choices themselves, and remembers the choice per workspace in that browser.

The app's own views stay general. Anything specific to one problem, such as where sudoku's boxes are, belongs in a drawing.

```ts
export function draw(world: World): string {
  return `<svg>...</svg>`; // or any HTML
}
```

## What it's given

`draw` gets what's still possible, already narrowed to whatever is pinned, so it can't disagree with the rest of the app.

```ts
type World = {
  choices: {
    id: string;
    name: string;
    options: { id: string; name: string; possible: boolean; picked: boolean; gap: boolean; weight: number }[];
  }[];
  count: string;         // How many worlds are left, as text because it can be enormous.
  constrained: boolean;  // Whether anything rules worlds out.
  solutions: Record<string, string>[]; // The worlds in play, if there's a requirement. The first 300.
  picks: Record<string, string>;       // What's pinned.
  nodes: { id: string; name: string; cells: Record<string, Cell> }[]; // Every node, with what the app shows of its cells.
  edges: { id: string; from: string; to: string; components: string[]; cells: Record<string, Cell> }[];
};
```

A `Cell` has the `value` it holds, a `formula` if one backs it, and where it comes from (`source` is `own`, `kind` or `default`). An option's `weight` is a cell of its `alternative` edge.

A choice's `id` is its node's, so `world.nodes.find((n) => n.id === choice.id)?.cells.at?.value` is where it goes on a board. Types are stripped, not checked, so `World` doesn't have to be declared.

## What it can do

An element with `data-pin-choice` and `data-pin-option` pins that option when it's clicked, and unpins it if it's already pinned. That's all a drawing can ask for.

Colours follow the theme as CSS variables: `--text`, `--muted`, `--accent`, `--bg`, `--panel`, `--border`, `--ok`, `--warn` and `--danger`.

If it throws, doesn't parse or takes more than 2 seconds, the worlds panel says why and shows the choices instead.

## What it can't do

A drawing is code that can arrive from a `git pull` or from Claude, and the local server has no login, so it's given nothing:

| Layer | What it does |
|---|---|
| A sandboxed frame with no origin of its own | It can't read the app or its cookies, or call its API as the app |
| A content security policy with no network | Nothing can be fetched or loaded, including from what it returns |
| A worker with no DOM, new for every draw | It remembers nothing between worlds, nothing keeps running after it answers, and it's stopped if it takes too long, so a loop can't hang the app |
| Cleaning what it returns | Scripts, event handlers, frames, forms, and links or sources that aren't `#` or an inline image are removed |
| Checking its messages | A pin has to name a real choice and option. Nothing else it sends can change anything |

Checked in the browser with drawings trying `fetch`, WebSocket, XHR, EventSource, `importScripts`, `sendBeacon`, images, stylesheets, CSS imports, SVG images, forms and links against a listener on another port. A request from the app itself reached it, and none from a drawing did. There's no automated test of this, because the web app has no test runner.

## Limits

- **No memory between worlds,** so no animation or state. Everything has to come from `world`.
- **Reading a drawing's source doesn't tell you it's correct.** A drawing could show something different from the real values. It's only a picture of them, and the default view is one menu choice away.
- **A person still has to read it as code** to trust it. The sandbox limits what it can do, not whether it's misleading.

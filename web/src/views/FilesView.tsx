import { useEffect, useState } from "react";
import { api, type FileInfo, type FileText } from "../api";
import type { ViewProps } from "./MapView";

const NODE_FILE = /^nodes\/(.+)\.yaml$/;

function sizeText(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** The workspace's files as they are on disk, to read and nothing else. It's for debugging. */
export function FilesView({ ws, selected }: ViewProps) {
  const [files, setFiles] = useState<FileInfo[]>([]);
  const [wanted, setWanted] = useState(selected && ws.byId.has(selected) ? `nodes/${selected}.yaml` : "recollect.toml");
  const [open, setOpen] = useState<FileText | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");

  // Anything that changes the graph changes the files.
  const { view } = ws;
  useEffect(() => {
    let stale = false;
    const failed = (e: unknown) => {
      if (stale) return;
      setOpen(null);
      setError(e instanceof Error ? e.message : String(e));
    };
    // Apart, because the one you're reading may be the file that's just gone.
    api.files().then((list) => {
      if (!stale) setFiles(list);
    }, failed);
    api.file(wanted).then((file) => {
      if (stale) return;
      setOpen(file);
      setError(null);
    }, failed);
    return () => {
      stale = true;
    };
  }, [wanted, view]);

  const nameOf = (path: string) => {
    const id = path.match(NODE_FILE)?.[1];
    return id ? ws.byId.get(id)?.name : undefined;
  };
  const needle = filter.trim().toLowerCase();
  const shown = files.filter((f) => !needle || f.path.toLowerCase().includes(needle) || nameOf(f.path)?.toLowerCase().includes(needle));

  return (
    <div className="files">
      <div className="files-list">
        <input className="files-filter" placeholder="Filter by path or name…" value={filter} onChange={(e) => setFilter(e.target.value)} />
        <div className="files-rows">
          {shown.map((f) => (
            <button key={f.path} className={`files-row ${f.path === wanted ? "selected" : ""}`} onClick={() => setWanted(f.path)} title={f.path}>
              <span className="files-name">{nameOf(f.path) ?? f.path}</span>
              <span className="muted small">{sizeText(f.size)}</span>
            </button>
          ))}
          {shown.length === 0 && <p className="muted small pad">Nothing matches.</p>}
        </div>
      </div>
      <div className="files-content">
        <div className="files-head muted small">
          <code>{wanted}</code>
          {open && <span> · {sizeText(open.size)}</span>}
          <span> · read-only</span>
        </div>
        {error && <div className="error">{error}</div>}
        {open?.truncated && <div className="muted small files-note">Only the start of this file is shown.</div>}
        {open && (open.text === null ? <p className="muted pad">This file isn't text.</p> : <pre className="files-text">{open.text}</pre>)}
      </div>
    </div>
  );
}

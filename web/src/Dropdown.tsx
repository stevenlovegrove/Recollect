import { createContext, useContext, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

const DropdownClose = createContext<() => void>(() => {});

/** A chip that opens a small popover; closes on outside click or Escape. */
export function Dropdown({ label, active, children }: { label: ReactNode; active?: boolean; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const close = () => setOpen(false);
  const ref = useRef<HTMLDivElement>(null);
  const pop = useRef<HTMLDivElement>(null);
  // Stay on screen: slide back from the right edge, and scroll if it's taller than what's left.
  useLayoutEffect(() => {
    const el = pop.current;
    if (!open || !el) return;
    const margin = 8;
    const box = el.getBoundingClientRect();
    const over = box.right - (window.innerWidth - margin);
    if (over > 0) el.style.marginLeft = `-${Math.min(over, box.left - margin)}px`;
    el.style.maxHeight = `${Math.max(120, window.innerHeight - box.top - margin)}px`;
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);
  return (
    <div className="dropdown" ref={ref}>
      <button className={`dd-chip ${active ? "active" : ""} ${open ? "open" : ""}`} onClick={() => setOpen(!open)}>
        {label} <span className="dd-caret">▾</span>
      </button>
      {open && (
        <DropdownClose.Provider value={close}>
          <div className="dd-pop" ref={pop} style={{ overflowY: "auto" }}>
            {children}
          </div>
        </DropdownClose.Provider>
      )}
    </div>
  );
}

/** A checkbox row inside a Dropdown. */
export function DdItem({
  checked,
  onChange,
  color,
  children,
  title,
  className,
}: {
  checked: boolean;
  onChange: () => void;
  color?: string;
  children: ReactNode;
  title?: string;
  className?: string;
}) {
  return (
    <label className={`dd-item ${className ?? ""}`} title={title} style={color ? { ["--c" as string]: color } : undefined}>
      <input type="checkbox" checked={checked} onChange={onChange} />
      {children}
    </label>
  );
}

/** A clickable row inside a Dropdown that closes it. */
export function DdItemButton({ onClick, color, children }: { onClick: () => void; color?: string; children: ReactNode }) {
  const close = useContext(DropdownClose);
  return (
    <button
      className="dd-item dd-btn"
      style={color ? { ["--c" as string]: color } : undefined}
      onClick={() => {
        close();
        onClick();
      }}
    >
      {children}
    </button>
  );
}

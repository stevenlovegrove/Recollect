import { useEffect, useRef, useState } from "react";

type Question = { text: string; placeholder?: string; confirm: string; danger?: boolean; needsReason: boolean; resolve: (answer: string | null) => void };

let show: (q: Question) => void = () => {};

/** Ask why something's being done. The answer goes in the log. `null` means they thought better of it. */
export function askWhy(text: string, confirm = "OK"): Promise<string | null> {
  return new Promise((resolve) => show({ text, placeholder: "Why? This is recorded in the log.", confirm, needsReason: true, resolve }));
}

/** Check they mean it. */
export function askSure(text: string, confirm: string): Promise<boolean> {
  return new Promise((resolve) => show({ text, confirm, danger: true, needsReason: false, resolve: (a) => resolve(a !== null) }));
}

/** Where questions appear. There's one, at the top of the app. */
export function AskHost() {
  const [q, setQ] = useState<Question | null>(null);
  const [text, setText] = useState("");
  const input = useRef<HTMLTextAreaElement>(null);
  show = (next) => {
    setText("");
    setQ(next);
  };
  useEffect(() => {
    if (q) input.current?.focus();
  }, [q]);
  if (!q) return null;
  const answer = (a: string | null) => {
    setQ(null);
    q.resolve(a);
  };
  return (
    <div className="modal-backdrop" onClick={() => answer(null)}>
      <form
        className="modal ask"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") answer(null);
          else if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            answer(text.trim());
          }
        }}
        onSubmit={(e) => {
          e.preventDefault();
          answer(text.trim());
        }}
      >
        <p>{q.text}</p>
        {q.needsReason && <textarea ref={input} rows={2} placeholder={q.placeholder} value={text} onChange={(e) => setText(e.target.value)} />}
        <div className="row end">
          <button type="button" onClick={() => answer(null)}>
            Cancel
          </button>
          <button autoFocus={!q.needsReason} className={q.danger ? "danger" : "primary"}>
            {q.confirm}
          </button>
        </div>
      </form>
    </div>
  );
}

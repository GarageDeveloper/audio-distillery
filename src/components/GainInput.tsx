import { useEffect, useState } from "react";

interface Props {
  /** Current value in dB, or null when unset (override cleared). */
  value: number | null;
  /** Shown when value is null (e.g. the inherited session gain). */
  placeholder?: string;
  /** Commit a parsed value; null = cleared (only when clearable). */
  onCommit: (v: number | null) => void;
  clearable?: boolean;
  title?: string;
}

const fmt = (v: number) =>
  v <= -60 ? "-60" : `${v > 0 ? "+" : ""}${Number(v.toFixed(1))}`;

/** Small dB text field: Enter/blur commits, Esc reverts, clamped -60..+12. */
export function GainInput({ value, placeholder, onCommit, clearable, title }: Props) {
  const [draft, setDraft] = useState(value == null ? "" : fmt(value));
  useEffect(() => {
    setDraft(value == null ? "" : fmt(value));
  }, [value]);

  const commit = () => {
    const text = draft.trim().replace(",", ".");
    if (text === "") {
      if (clearable) onCommit(null);
      else setDraft(value == null ? "" : fmt(value));
      return;
    }
    const parsed = Number(text);
    if (Number.isFinite(parsed)) {
      onCommit(Math.min(12, Math.max(-60, parsed)));
    } else {
      setDraft(value == null ? "" : fmt(value));
    }
  };

  return (
    <input
      className="gain-input"
      value={draft}
      placeholder={placeholder}
      title={title ?? "Gain in dB (-60 to +12)"}
      onChange={(e) => setDraft(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        else if (e.key === "Escape") {
          setDraft(value == null ? "" : fmt(value));
          (e.target as HTMLInputElement).blur();
        }
      }}
      onBlur={commit}
    />
  );
}

import { useEffect, useRef, useState } from "react";
import type { ProjectView } from "../types/ProjectView";
import { GainInput } from "./GainInput";

interface Props {
  view: ProjectView;
  onRename: (id: number, name: string) => void;
  onGain: (id: number, gainDb: number) => void;
  onMute: (id: number, muted: boolean) => void;
  onSolo: (id: number, solo: boolean) => void;
  onCollapse: (id: number, collapsed: boolean) => void;
  onRemove: (id: number) => void;
  onAdd: () => void;
}

/**
 * Mix section: one row per time-synchronized layer (field-recorder inputs…) with a
 * gain fader, mute and remove. Values are the backend's; slider moves are
 * throttled intentions.
 */
export function LayersPanel({ view, onRename, onGain, onMute, onSolo, onCollapse, onRemove, onAdd }: Props) {
  const throttle = useRef<Record<number, number>>({});
  const [editing, setEditing] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editing != null) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  const commit = (id: number, nextId?: number) => {
    onRename(id, draft);
    if (nextId !== undefined) {
      const next = view.layers.find((l) => l.id === nextId);
      if (next) {
        setDraft(next.name);
        setEditing(nextId);
        return;
      }
    }
    setEditing(null);
  };

  const sendGain = (id: number, value: number) => {
    const now = performance.now();
    const last = throttle.current[id] ?? 0;
    if (now - last >= 80) {
      throttle.current[id] = now;
      onGain(id, value);
    }
  };

  return (
    <div className="layers-section">
      <div className="layers-head">
        <span className="label">Layers</span>
        <button className="btn btn-icon" onClick={onAdd} title="Add synced layers (files starting at the same instant)">
          +
        </button>
      </div>
      {view.layers.map((l, i) => (
        <div key={l.id} className={`layer-row ${l.muted ? "muted" : ""}`}>
          <div className="layer-top">
            <button
              className="layer-chevron"
              title={l.collapsed ? "Expand this lane in the Layers view" : "Collapse this lane to a thin strip in the Layers view"}
              onClick={() => onCollapse(l.id, !l.collapsed)}
            >
              {l.collapsed ? "▸" : "▾"}
            </button>
            {editing === l.id ? (
              <input
                ref={inputRef}
                className="rename-input layer-rename"
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onBlur={() => commit(l.id)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commit(l.id);
                  if (e.key === "Escape") setEditing(null);
                  if (e.key === "Tab") {
                    e.preventDefault();
                    commit(l.id, view.layers[i + 1]?.id);
                  }
                }}
              />
            ) : (
              <span
                className="layer-name"
                title={`${l.source_name} — double-click to rename`}
                onDoubleClick={() => {
                  setDraft(l.name);
                  setEditing(l.id);
                }}
              >
                {l.name}
              </span>
            )}
            <span className="layer-ch">{l.channels === 1 ? "mono" : "stereo"}</span>
            <button
              className={`layer-mute ${l.muted ? "on" : ""}`}
              title={l.muted ? "Unmute" : "Mute"}
              onClick={() => onMute(l.id, !l.muted)}
            >
              M
            </button>
            <button
              className={`layer-solo ${l.solo ? "on" : ""}`}
              title={l.solo ? "Unsolo" : "Solo — only soloed layers are audible"}
              onClick={() => onSolo(l.id, !l.solo)}
            >
              S
            </button>
            <button
              className="layer-del"
              title={i === 0 ? "The base layer cannot be removed" : "Remove this layer"}
              disabled={i === 0}
              onClick={() => onRemove(l.id)}
            >
              ✕
            </button>
          </div>
          <div className="layer-fader">
            <input
              type="range"
              min={-60}
              max={12}
              step={0.5}
              value={l.gain_db}
              onChange={(e) => sendGain(l.id, Number(e.target.value))}
              onPointerUp={(e) => onGain(l.id, Number((e.target as HTMLInputElement).value))}
              onDoubleClick={() => onGain(l.id, 0)}
              title="Drag to set the mix gain — double-click to reset to 0 dB"
            />
            <GainInput
              value={l.gain_db}
              onCommit={(v) => v != null && onGain(l.id, v)}
              title="Type the mix gain in dB (-60 to +12), Enter to apply"
            />
            <span className="layer-db-unit">dB</span>
          </div>
        </div>
      ))}
    </div>
  );
}

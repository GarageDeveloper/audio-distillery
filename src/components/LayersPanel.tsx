import { useRef } from "react";
import type { ProjectView } from "../types/ProjectView";

interface Props {
  view: ProjectView;
  onGain: (id: number, gainDb: number) => void;
  onMute: (id: number, muted: boolean) => void;
  onRemove: (id: number) => void;
  onAdd: () => void;
}

/**
 * Mix section: one row per time-synchronized layer (Zoom inputs…) with a
 * gain fader, mute and remove. Values are the backend's; slider moves are
 * throttled intentions.
 */
export function LayersPanel({ view, onGain, onMute, onRemove, onAdd }: Props) {
  const throttle = useRef<Record<number, number>>({});

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
            <span className="layer-name" title={l.name}>
              {l.name}
            </span>
            <span className="layer-ch">{l.channels === 1 ? "mono" : "stereo"}</span>
            <button
              className={`layer-mute ${l.muted ? "on" : ""}`}
              title={l.muted ? "Unmute" : "Mute"}
              onClick={() => onMute(l.id, !l.muted)}
            >
              M
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
            <span className="layer-db">
              {l.gain_db <= -60 ? "-∞" : `${l.gain_db > 0 ? "+" : ""}${l.gain_db.toFixed(1)}`} dB
            </span>
          </div>
        </div>
      ))}
    </div>
  );
}

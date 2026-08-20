import { useEffect, useMemo, useState } from "react";
import type { ProjectView } from "../types/ProjectView";
import type { AuComponentInfo } from "../types/AuComponentInfo";
import { api } from "../api";
import { Backdrop } from "./Backdrop";

interface Props {
  view: ProjectView;
  onClose: () => void;
  onError: (msg: string) => void;
  onViewChange: (v: ProjectView) => void;
}

/**
 * Master-bus mastering chain: ordered AU plugins processed live in the
 * playback engine. Add from the installed-effects browser, reorder, bypass
 * (live), remove. Plugin state is saved with the project.
 */
export function MasteringDialog({ view, onClose, onError, onViewChange }: Props) {
  const [available, setAvailable] = useState<AuComponentInfo[]>([]);
  const [filter, setFilter] = useState("");
  const [browserOpen, setBrowserOpen] = useState(view.mastering_chain.length === 0);

  useEffect(() => {
    api.listAudioUnits().then(setAvailable).catch((e) => onError(String(e)));
  }, [onError]);

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return available;
    return available.filter((a) => a.name.toLowerCase().includes(q));
  }, [available, filter]);

  const run = (op: () => Promise<ProjectView>) => {
    op().then(onViewChange).catch((e) => onError(String(e)));
  };

  const chain = view.mastering_chain;

  return (
    <Backdrop onClose={() => {}}>
      <div className="modal export-modal">
        <div>
          <h2>Mastering</h2>
          <div className="subtitle">
            Master-bus plugin chain — processed live during playback, saved in the project
          </div>
        </div>

        <div className="field">
          <label>Chain ({chain.length})</label>
          {chain.length === 0 && (
            <div className="hint">No plugins yet — pick one below to start the chain.</div>
          )}
          <div className="chain-list">
            {chain.map((p, i) => (
              <div key={p.id} className={`chain-row ${p.bypass ? "bypassed" : ""}`}>
                <span className="chain-num">{i + 1}</span>
                <span className="chain-name" title={p.component}>
                  {p.name}
                </span>
                <button
                  className="btn chain-edit"
                  title="Open the plugin's editor window"
                  onClick={() => {
                    api.openPluginEditor(p.id).catch((e) => onError(String(e)));
                  }}
                >
                  Edit
                </button>
                <button
                  className="disc-sep-btn"
                  disabled={i === 0}
                  title="Move earlier in the chain"
                  onClick={() => run(() => api.moveMasteringPlugin(p.id, -1))}
                >
                  ↑
                </button>
                <button
                  className="disc-sep-btn"
                  disabled={i === chain.length - 1}
                  title="Move later in the chain"
                  onClick={() => run(() => api.moveMasteringPlugin(p.id, 1))}
                >
                  ↓
                </button>
                <button
                  className={`layer-mute ${p.bypass ? "on" : ""}`}
                  title={p.bypass ? "Re-enable (live)" : "Bypass (live)"}
                  onClick={() => run(() => api.setMasteringBypass(p.id, !p.bypass))}
                >
                  B
                </button>
                <button
                  className="del chain-del"
                  title="Remove from the chain"
                  onClick={() => run(() => api.removeMasteringPlugin(p.id))}
                >
                  ✕
                </button>
              </div>
            ))}
          </div>
        </div>

        <div className="field">
          <button
            className="meta-toggle"
            onClick={() => setBrowserOpen(!browserOpen)}
            aria-expanded={browserOpen}
          >
            {browserOpen ? "▾" : "▸"} Add a plugin
            <span className="hint-inline">{available.length} Audio Units installed</span>
          </button>
          {browserOpen && (
            <>
              <input
                className="text-input"
                placeholder="Filter by name…"
                value={filter}
                autoFocus
                onChange={(e) => setFilter(e.target.value)}
              />
              <div className="plugin-browser">
                {filtered.map((a) => (
                  <button
                    key={a.id}
                    className="plugin-item"
                    title={a.id}
                    onClick={() => run(() => api.addMasteringPlugin(a.id, a.name))}
                  >
                    {a.name}
                  </button>
                ))}
                {filtered.length === 0 && (
                  <div className="hint">No match — is the plugin installed as an Audio Unit?</div>
                )}
              </div>
            </>
          )}
        </div>

        <div className="hint">
          Edit opens the plugin's own window (or a generic parameter view) — tweak while
          playing. Settings are saved in the project (⌘S). Note: reordering or
          removing plugins closes open editors; the export render through the chain
          arrives in the next step.
        </div>

        <div className="modal-foot">
          <button className="btn btn-primary" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </Backdrop>
  );
}

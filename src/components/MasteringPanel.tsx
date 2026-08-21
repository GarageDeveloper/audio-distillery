import { useEffect, useMemo, useState } from "react";
import type { ProjectView } from "../types/ProjectView";
import type { AuComponentInfo } from "../types/AuComponentInfo";
import { api } from "../api";

interface Props {
  view: ProjectView;
  onError: (msg: string) => void;
  onViewChange: (v: ProjectView) => void;
}

const RECENT_KEY = "still-recent-plugins";

function loadRecent(): string[] {
  try {
    return JSON.parse(localStorage.getItem(RECENT_KEY) ?? "[]");
  } catch {
    return [];
  }
}

function pushRecent(id: string) {
  const next = [id, ...loadRecent().filter((r) => r !== id)].slice(0, 6);
  localStorage.setItem(RECENT_KEY, JSON.stringify(next));
}

/**
 * Channel-strip style mastering column: the plugin chain as vertical slots
 * (click a slot = open its editor), plus a Logic-inspired hierarchical
 * picker: search field, Recent, then manufacturer → plugins drill-down.
 */
export function MasteringPanel({ view, onError, onViewChange }: Props) {
  const [available, setAvailable] = useState<AuComponentInfo[]>([]);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [filter, setFilter] = useState("");
  const [maker, setMaker] = useState<string | null>(null);
  const [recent, setRecent] = useState<string[]>(loadRecent());

  useEffect(() => {
    api.listAudioUnits().then(setAvailable).catch((e) => onError(String(e)));
  }, [onError]);

  const run = (op: () => Promise<ProjectView>) => {
    op().then(onViewChange).catch((e) => onError(String(e)));
  };

  const add = (a: AuComponentInfo) => {
    pushRecent(a.id);
    setRecent(loadRecent());
    setPickerOpen(false);
    setFilter("");
    setMaker(null);
    run(() => api.addMasteringPlugin(a.id, a.name));
  };

  const chain = view.mastering_chain;
  const query = filter.trim().toLowerCase();

  const makers = useMemo(() => {
    const m = new Map<string, number>();
    for (const a of available) {
      const key = a.manufacturer || "Other";
      m.set(key, (m.get(key) ?? 0) + 1);
    }
    return [...m.entries()].sort((x, y) => x[0].localeCompare(y[0]));
  }, [available]);

  const searchResults = useMemo(() => {
    if (!query) return [];
    return available
      .filter(
        (a) =>
          a.name.toLowerCase().includes(query) ||
          a.manufacturer.toLowerCase().includes(query)
      )
      .slice(0, 40);
  }, [available, query]);

  const recentInfos = useMemo(
    () =>
      recent
        .map((id) => available.find((a) => a.id === id))
        .filter((a): a is AuComponentInfo => !!a),
    [recent, available]
  );

  return (
    <aside className="mastering-panel">
      <div className="track-panel-head">
        <span className="label">Mastering</span>
        {chain.length > 0 && (
          <button
            className="disc-sep-btn reload-btn"
            title="Re-instantiate every plugin with its current settings"
            onClick={() => run(() => api.reloadMasteringChain())}
          >
            ⟳
          </button>
        )}
      </div>

      <div className="strip-slots">
        {chain.map((p, i) => (
          <div key={p.id} className={`strip-slot ${p.bypass ? "bypassed" : ""}`}>
            <button
              className="strip-name"
              title={`${p.name} — click to open the editor`}
              onClick={() => api.openPluginEditor(p.id).catch((e) => onError(String(e)))}
            >
              {p.name}
            </button>
            <div className="strip-actions">
              <button
                className="disc-sep-btn"
                disabled={i === 0}
                title="Move earlier"
                onClick={() => run(() => api.moveMasteringPlugin(p.id, -1))}
              >
                ↑
              </button>
              <button
                className="disc-sep-btn"
                disabled={i === chain.length - 1}
                title="Move later"
                onClick={() => run(() => api.moveMasteringPlugin(p.id, 1))}
              >
                ↓
              </button>
              <button
                className={`layer-mute ${p.bypass ? "on" : ""}`}
                title={p.bypass ? "Re-enable" : "Bypass"}
                onClick={() => run(() => api.setMasteringBypass(p.id, !p.bypass))}
              >
                B
              </button>
              <button
                className="del strip-del"
                title="Remove"
                onClick={() => run(() => api.removeMasteringPlugin(p.id))}
              >
                ✕
              </button>
            </div>
          </div>
        ))}

        {!pickerOpen ? (
          <button className="strip-add" onClick={() => setPickerOpen(true)}>
            + Add plugin
          </button>
        ) : (
          <div className="plugin-picker">
            <input
              className="text-input"
              placeholder="Search plugins…"
              value={filter}
              autoFocus
              onChange={(e) => setFilter(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  setPickerOpen(false);
                  setFilter("");
                  setMaker(null);
                }
              }}
            />

            {query ? (
              <div className="picker-list">
                {searchResults.map((a) => (
                  <button key={a.id} className="picker-item" onClick={() => add(a)}>
                    <span className="picker-name">{a.name}</span>
                    <span className="picker-maker">{a.manufacturer}</span>
                  </button>
                ))}
                {searchResults.length === 0 && <div className="hint">No match.</div>}
              </div>
            ) : maker === null ? (
              <div className="picker-list">
                {recentInfos.length > 0 && (
                  <>
                    <div className="picker-section">Recent</div>
                    {recentInfos.map((a) => (
                      <button key={a.id} className="picker-item" onClick={() => add(a)}>
                        <span className="picker-name">{a.name}</span>
                        <span className="picker-maker">{a.manufacturer}</span>
                      </button>
                    ))}
                    <div className="picker-section">Audio Units</div>
                  </>
                )}
                {makers.map(([m, count]) => (
                  <button key={m} className="picker-item" onClick={() => setMaker(m)}>
                    <span className="picker-name">{m}</span>
                    <span className="picker-maker">{count} ›</span>
                  </button>
                ))}
              </div>
            ) : (
              <div className="picker-list">
                <button className="picker-item picker-back" onClick={() => setMaker(null)}>
                  ‹ {maker}
                </button>
                {available
                  .filter((a) => (a.manufacturer || "Other") === maker)
                  .map((a) => (
                    <button key={a.id} className="picker-item" onClick={() => add(a)}>
                      <span className="picker-name">{a.name}</span>
                    </button>
                  ))}
              </div>
            )}

            <button
              className="btn picker-cancel"
              onClick={() => {
                setPickerOpen(false);
                setFilter("");
                setMaker(null);
              }}
            >
              Cancel
            </button>
          </div>
        )}
      </div>

      <div className="track-panel-foot">
        <span>Click a slot to edit</span>
        <span>{chain.length} plugin{chain.length !== 1 ? "s" : ""}</span>
      </div>
    </aside>
  );
}

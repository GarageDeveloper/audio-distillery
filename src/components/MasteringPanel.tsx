import { useEffect, useMemo, useRef, useState } from "react";
import type { ProjectView } from "../types/ProjectView";
import type { PluginInfo } from "../types/PluginInfo";
import type { ChainPresetInfo } from "../types/ChainPresetInfo";
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
  const [available, setAvailable] = useState<PluginInfo[]>([]);
  const [pickerOpen, setPickerOpen] = useState(false);
  const [filter, setFilter] = useState("");
  const [maker, setMaker] = useState<string | null>(null);
  const [recent, setRecent] = useState<string[]>(loadRecent());
  const [presetsOpen, setPresetsOpen] = useState(false);
  const [presets, setPresets] = useState<ChainPresetInfo[]>([]);
  const [presetName, setPresetName] = useState("");
  // Pointer-based slot dragging (HTML5 DnD is owned by Tauri's file drop).
  const slotsRef = useRef<HTMLDivElement | null>(null);
  const [drag, setDrag] = useState<{
    from: number;
    over: number;
    x: number;
    y: number;
  } | null>(null);

  useEffect(() => {
    api.listPlugins().then(setAvailable).catch((e) => onError(String(e)));
  }, [onError]);

  const run = (op: () => Promise<ProjectView>) => {
    op().then(onViewChange).catch((e) => onError(String(e)));
  };

  const add = (a: PluginInfo) => {
    pushRecent(a.id);
    setRecent(loadRecent());
    setPickerOpen(false);
    setFilter("");
    setMaker(null);
    run(() => api.addMasteringPlugin(a.id, a.name));
  };

  const chain = view.mastering_chain;
  const query = filter.trim().toLowerCase();

  const togglePresets = () => {
    const opening = !presetsOpen;
    setPresetsOpen(opening);
    if (opening) {
      api.listChainPresets().then(setPresets).catch((e) => onError(String(e)));
    }
  };

  const savePreset = () => {
    const name = presetName.trim();
    if (!name || chain.length === 0) return;
    api
      .saveChainPreset(name)
      .then((list) => {
        setPresets(list);
        setPresetName("");
      })
      .catch((e) => onError(String(e)));
  };

  // Drag gesture, same pattern as the waveform markers: pointer capture on
  // the slot + move/up handlers ON THE SLOT ITSELF (capture retargets every
  // pointer event to it). The gesture lives in a ref; `drag` state only
  // drives the visuals. A press that never crosses the 5px threshold is a
  // click, synthesized on release (capture eats real click events).
  const gesture = useRef<{
    from: number;
    id: number;
    startY: number;
    clickedName: boolean;
    started: boolean;
  } | null>(null);

  const insertionAt = (y: number, fallback: number) => {
    const els = slotsRef.current?.querySelectorAll<HTMLElement>(".strip-slot");
    if (!els) return fallback;
    let over = els.length;
    for (let i = 0; i < els.length; i++) {
      const r = els[i].getBoundingClientRect();
      if (y < r.top + r.height / 2) {
        over = i;
        break;
      }
    }
    return over;
  };

  const onSlotPointerDown = (e: React.PointerEvent, from: number, id: number) => {
    if (e.button !== 0) return;
    if ((e.target as HTMLElement).closest(".strip-actions")) return;
    // Suppress the compatibility mouse events: WebKit's native press
    // tracking (text hit-testing) otherwise swallows every pointermove
    // when the press starts over the name's text.
    e.preventDefault();
    gesture.current = {
      from,
      id,
      startY: e.clientY,
      clickedName: !!(e.target as HTMLElement).closest(".strip-name"),
      started: false,
    };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  };

  const onSlotPointerMove = (e: React.PointerEvent) => {
    const g = gesture.current;
    if (!g) return;
    if (!g.started) {
      if (Math.abs(e.clientY - g.startY) < 5) return;
      g.started = true;
    }
    setDrag({
      from: g.from,
      over: insertionAt(e.clientY, g.from),
      x: e.clientX,
      y: e.clientY,
    });
  };

  const onSlotPointerUp = (e: React.PointerEvent) => {
    const g = gesture.current;
    gesture.current = null;
    setDrag(null);
    if (!g) return;
    if (g.started) {
      const over = insertionAt(e.clientY, g.from);
      const to = over > g.from ? over - 1 : over;
      if (to !== g.from) run(() => api.moveMasteringPlugin(g.id, to - g.from));
    } else if (g.clickedName) {
      api.openPluginEditor(g.id).catch((err) => onError(String(err)));
    }
  };

  const onSlotPointerCancel = () => {
    gesture.current = null;
    setDrag(null);
  };

  const makers = useMemo(() => {
    const m = new Map<string, number>();
    for (const a of available) {
      const key = a.manufacturer || "Other";
      m.set(key, (m.get(key) ?? 0) + 1);
    }
    return [...m.entries()].sort((x, y) => x[0].localeCompare(y[0]));
  }, [available]);

  // Same plugin in both formats: sort by name, AU right above VST3.
  const byNameThenFormat = (a: PluginInfo, b: PluginInfo) =>
    a.name.toLowerCase().localeCompare(b.name.toLowerCase()) ||
    (a.format === b.format ? 0 : a.format === "au" ? -1 : 1);

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
        .filter((a): a is PluginInfo => !!a),
    [recent, available]
  );

  return (
    <aside className="mastering-panel">
      <div className="track-panel-head">
        <span className="label">Mastering</span>
        <div className="strip-head-actions">
          <button
            className={`disc-sep-btn ${presetsOpen ? "active" : ""}`}
            title="Saved chains (presets)"
            onClick={togglePresets}
          >
            ▾
          </button>
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
      </div>

      {presetsOpen && (
        <div className="presets-menu">
          {presets.length === 0 ? (
            <div className="hint">No saved chains yet.</div>
          ) : (
            <div className="preset-list">
              {presets.map((p) => (
                <div
                  key={p.name}
                  className="preset-item"
                  title={p.plugins.join(" → ")}
                >
                  <button
                    className="preset-load"
                    onClick={() => {
                      setPresetsOpen(false);
                      run(() => api.loadChainPreset(p.name));
                    }}
                  >
                    <span className="picker-name">{p.name}</span>
                    <span className="picker-maker">{p.plugins.length}</span>
                  </button>
                  <button
                    className="del preset-del"
                    title="Delete this preset"
                    onClick={() =>
                      api
                        .deleteChainPreset(p.name)
                        .then(setPresets)
                        .catch((e) => onError(String(e)))
                    }
                  >
                    ✕
                  </button>
                </div>
              ))}
            </div>
          )}
          <div className="preset-save">
            <input
              className="text-input"
              placeholder="Save chain as…"
              value={presetName}
              onChange={(e) => setPresetName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") savePreset();
                if (e.key === "Escape") setPresetsOpen(false);
              }}
            />
            <button
              className="btn"
              disabled={!presetName.trim() || chain.length === 0}
              title={
                chain.length === 0
                  ? "The chain is empty"
                  : "Save the current chain (plugin settings included)"
              }
              onClick={savePreset}
            >
              Save
            </button>
          </div>
        </div>
      )}

      <div className="strip-slots" ref={slotsRef}>
        {chain.map((p, i) => (
          <div
            key={p.id}
            className={[
              "strip-slot",
              p.bypass ? "bypassed" : "",
              drag?.from === i ? "dragging" : "",
              drag && drag.over === i && drag.from !== i ? "drop-before" : "",
              drag && drag.over === chain.length && i === chain.length - 1 && drag.from !== i
                ? "drop-after"
                : "",
            ].join(" ")}
            onPointerDown={(e) => onSlotPointerDown(e, i, p.id)}
            onPointerMove={onSlotPointerMove}
            onPointerUp={onSlotPointerUp}
            onPointerCancel={onSlotPointerCancel}
          >
            {/* Deliberately NOT a <button>: WebKit's native form-control
                tracking swallows pointermove during a press on a button,
                which killed dragging everywhere except the card's edges.
                The editor-open "click" is synthesized on pointerup. */}
            <div
              className="strip-name"
              title={`${p.name} — click to open the editor, drag to reorder`}
            >
              <span className="strip-name-text">{p.name}</span>
              <span className="picker-format">
                {p.format === "vst3" ? "VST3" : "AU"}
              </span>
            </div>
            <div className="strip-actions">
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
                    <span className="picker-maker">
                      {a.manufacturer}
                      <span className="picker-format">
                        {a.format === "vst3" ? "VST3" : "AU"}
                      </span>
                    </span>
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
                        <span className="picker-maker">
                          {a.manufacturer}
                          <span className="picker-format">
                            {a.format === "vst3" ? "VST3" : "AU"}
                          </span>
                        </span>
                      </button>
                    ))}
                  </>
                )}
                <div className="picker-section">Plugins</div>
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
                  .sort(byNameThenFormat)
                  .map((a) => (
                    <button key={a.id} className="picker-item" onClick={() => add(a)}>
                      <span className="picker-name">{a.name}</span>
                      <span className="picker-format">
                        {a.format === "vst3" ? "VST3" : "AU"}
                      </span>
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

      {drag && (
        <div
          className="strip-ghost"
          style={{ left: drag.x - 90, top: drag.y - 14 }}
        >
          {chain[drag.from]?.name}
        </div>
      )}

      <div className="track-panel-foot">
        <span>Click a slot to edit</span>
        <span>{chain.length} plugin{chain.length !== 1 ? "s" : ""}</span>
      </div>
    </aside>
  );
}

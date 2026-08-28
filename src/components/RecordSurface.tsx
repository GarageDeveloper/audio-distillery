import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";

import type { InputDeviceInfo } from "../types/InputDeviceInfo";
import type { RecordLane } from "../types/RecordLane";
import type { RecordStatus } from "../types/RecordStatus";
import { api } from "../api";

interface Props {
  /** A session is open — the surface sits above the dimmed timeline. */
  hasSession: boolean;
  onOpen: () => void;
  onError: (msg: string) => void;
  /** Called with the recorded file paths after a successful stop. */
  onRecorded: (paths: string[]) => void;
}

const dbOf = (peak: number) =>
  peak > 0 ? Math.max(-60, 20 * Math.log10(peak)) : -60;

const SETUP_KEY = "still-record-setup";

/** The Record phase, full pane: pick an interface, build the lane list,
 * watch the meters live (monitor mode — stream open, nothing written),
 * then track. Display only — arming, streaming, metering and file
 * writing are backend. */
export function RecordSurface({ hasSession, onOpen, onError, onRecorded }: Props) {
  const [devices, setDevices] = useState<InputDeviceInfo[]>([]);
  // Composite key: a card can appear under several hosts (WASAPI + ASIO).
  const keyOf = (d: { host: string; name: string }) => `${d.host}\u001f${d.name}`;
  const [device, setDevice] = useState<string>("");
  const [lanes, setLanes] = useState<RecordLane[]>([]);
  const [addFirst, setAddFirst] = useState(1);
  // Kept as text so backspace/delete behave; parsed when used.
  const [addCount, setAddCount] = useState("1");
  const addCountN = Math.max(1, parseInt(addCount, 10) || 1);
  const [destDir, setDestDir] = useState("");
  const [status, setStatus] = useState<RecordStatus | null>(null);
  const [starting, setStarting] = useState(false);
  // Max-hold per lane so short peaks stay visible between polls.
  const holds = useRef<number[]>([]);
  // Forces a paint between polls so the holds decay smoothly.
  const [, setMeterTick] = useState(0);
  // True once ANY lane has shown signal during this take.
  const sawSignal = useRef(false);
  // OS microphone authorization: granted | denied | undetermined |
  // restricted | unknown (platforms that don't expose it).
  const [micPerm, setMicPerm] = useState<string>("unknown");

  const recording = status?.recording ?? false;
  const monitoring = status?.monitoring ?? false;
  const live = recording || monitoring;
  const liveRef = useRef({ recording, monitoring });
  liveRef.current = { recording, monitoring };
  const selected = devices.find((d) => keyOf(d) === device);
  const multiHost = new Set(devices.map((d) => d.host)).size > 1;
  const channels = selected?.channels ?? 0;
  const inputName = (n: number) =>
    selected?.input_names[n - 1] ?? `Input ${n}`;
  const invalidLanes = channels > 0 && lanes.some((l) => l.input > channels);

  const adoptDevices = (list: InputDeviceInfo[]) => {
    setDevices(list);
    setDevice((cur) => {
      if (cur && list.some((d) => keyOf(d) === cur)) return cur;
      const def = list.find((d) => d.is_default) ?? list[0];
      return def ? keyOf(def) : "";
    });
  };

  useEffect(() => {
    api.listInputDevices().then(adoptDevices).catch(() => {});
    api
      .getDefaultRecordingDir()
      .then((d) => setDestDir((cur) => cur || d))
      .catch(() => {});
    // Adopt an already-running take if the phase was re-entered mid-take.
    api.recordStatus().then((s) => s && setStatus(s)).catch(() => {});
    api.micPermission().then(setMicPerm).catch(() => {});
    try {
      const saved = JSON.parse(localStorage.getItem(SETUP_KEY) ?? "null");
      if (saved?.lanes?.length) {
        setLanes(saved.lanes);
        if (saved.device) setDevice(saved.device);
      }
    } catch {
      /* fresh start */
    }
    return () => {
      // Leaving the phase closes the monitor stream — but NEVER a take:
      // a running recording keeps rolling behind any phase.
      if (liveRef.current.monitoring && !liveRef.current.recording) {
        void api.recordStop().catch(() => {});
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Track the authorization until it lands on granted — it changes
  // under our feet when the user answers the prompt or flips the
  // System Settings toggle.
  useEffect(() => {
    if (micPerm === "granted" || micPerm === "unknown") return;
    const t = window.setInterval(() => {
      api.micPermission().then(setMicPerm).catch(() => {});
    }, 1200);
    return () => window.clearInterval(t);
  }, [micPerm]);

  // Hot-plug: the backend watcher pushes an event ONLY when the
  // topology changes — no polling, nothing on the UI thread.
  useEffect(() => {
    const un = listen<InputDeviceInfo[]>("record:devices", (e) => adoptDevices(e.payload));
    return () => {
      void un.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // The monitor follows the setup: whenever the interface or the lane
  // list settles (and no take is running), (re)open a monitor stream so
  // every meter is live BEFORE the red button. Debounced — lane edits
  // come in bursts.
  useEffect(() => {
    if (recording || starting) return;
    if (!selected || lanes.length === 0 || invalidLanes || micPerm === "denied" || micPerm === "restricted") {
      if (liveRef.current.monitoring) {
        void api.recordStop().then(() => setStatus(null)).catch(() => {});
      }
      return;
    }
    const t = window.setTimeout(() => {
      api
        .recordMonitor({
          host: selected.host,
          device: selected.name,
          lanes,
          dest_dir: destDir,
        })
        .then((s) => {
          holds.current = [];
          setStatus(s);
        })
        .catch(() => {});
    }, 350);
    return () => window.clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [device, lanes, channels, recording, starting, micPerm]);

  // One status poll drives everything live: meters (monitor + take),
  // the clock, dropped frames.
  useEffect(() => {
    if (!live) return;
    const t = window.setInterval(() => {
      api
        .recordStatus()
        .then((s) => {
          if (s) {
            s.levels.forEach((v, i) => {
              holds.current[i] = Math.max(v, (holds.current[i] ?? 0) * 0.82);
            });
            if (s.recording && s.levels.some((v) => v > 0)) sawSignal.current = true;
            setStatus(s);
          } else {
            setStatus(null);
          }
          setMeterTick((n) => n + 1);
        })
        .catch(() => {});
    }, 150);
    return () => window.clearInterval(t);
  }, [live]);

  const persist = (dev: string, ls: RecordLane[]) => {
    try {
      localStorage.setItem(SETUP_KEY, JSON.stringify({ device: dev, lanes: ls }));
    } catch {
      /* best effort */
    }
  };
  const updateLanes = (ls: RecordLane[]) => {
    setLanes(ls);
    persist(device, ls);
  };

  const addBatch = () => {
    const fresh: RecordLane[] = [];
    for (let i = 0; i < addCountN; i++) {
      const input = addFirst + i;
      if (channels > 0 && input > channels) break;
      fresh.push({ input, name: inputName(input) });
    }
    updateLanes([...lanes, ...fresh]);
    setAddFirst(Math.min(addFirst + fresh.length, Math.max(channels, 1)));
  };

  const start = async () => {
    setStarting(true);
    holds.current = [];
    sawSignal.current = false;
    try {
      // The backend swaps the monitor out for the take atomically.
      const s = await api.recordStart({
        host: selected?.host ?? "",
        device: selected?.name ?? "",
        lanes,
        dest_dir: destDir,
      });
      setStatus(s);
    } catch (e) {
      onError(String(e));
    } finally {
      setStarting(false);
    }
  };

  const stop = async () => {
    try {
      const paths = await api.recordStop();
      setStatus(null);
      onRecorded(paths);
    } catch (e) {
      setStatus(null);
      onError(String(e));
    }
  };

  const elapsed = recording ? status?.elapsed_seconds ?? 0 : 0;
  const mm = Math.floor(elapsed / 60);
  const ss = Math.floor(elapsed % 60);

  return (
    <div className="record-surface">
      <div className="record-setup">
        {!hasSession && (
          <div className="record-open-hint">
            Starting from existing audio instead?{" "}
            <button className="btn" onClick={onOpen}>
              Open…
            </button>{" "}
            <span className="hint">or drop files anywhere.</span>
          </div>
        )}

        <div className="field">
          <label>Interface</label>
          <select
            className="select"
            value={device}
            disabled={starting || recording}
            onChange={(e) => {
              setDevice(e.target.value);
              persist(e.target.value, lanes);
            }}
          >
            {devices.length === 0 && <option value="">No input device found</option>}
            {devices.map((d) => (
              <option key={keyOf(d)} value={keyOf(d)}>
                {multiHost ? `[${d.host}] ` : ""}
                {d.name} — {d.channels} input{d.channels > 1 ? "s" : ""} @{" "}
                {(d.sample_rate / 1000).toLocaleString("en-US", {
                  maximumFractionDigits: 1,
                })}{" "}
                kHz
              </option>
            ))}
          </select>
          <div className="hint">The list refreshes itself when devices are plugged or unplugged.</div>
          {micPerm === "undetermined" && (
            <div className="mic-perm">
              macOS has not been asked for microphone access yet.
              <button
                className="btn"
                onClick={() => {
                  void api.requestMicAccess(false);
                }}
              >
                Grant access…
              </button>
            </div>
          )}
          {(micPerm === "denied" || micPerm === "restricted") && (
            <div className="mic-perm denied">
              Microphone access is {micPerm} for AudioDistillery — every
              input records silence.
              <span className="mic-perm-actions">
                <button
                  className="btn"
                  onClick={() =>
                    void openUrl(
                      "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
                    )
                  }
                >
                  Open System Settings
                </button>
                <button
                  className="btn"
                  title="Forget the previous decision for this app and show the consent dialog again"
                  onClick={() => {
                    void api.requestMicAccess(true);
                  }}
                >
                  Ask again
                </button>
              </span>
            </div>
          )}
        </div>

        <div className="field">
          <label>
            Lanes ({lanes.length})
            {monitoring && <span className="record-monitor-badge">monitoring</span>}
          </label>
          {lanes.length > 0 && (
            <div className="lane-list">
              {lanes.map((l, i) => {
                const missing = channels > 0 && l.input > channels;
                const hold = holds.current[i] ?? 0;
                const db = dbOf(hold);
                const pct = live ? Math.max(0, Math.min(100, ((db + 60) / 60) * 100)) : 0;
                return (
                  <div key={i} className={`lane-row ${missing ? "missing" : ""}`}>
                    <span className="lane-num">{String(i + 1).padStart(2, "0")}</span>
                    <select
                      className="select lane-input"
                      value={l.input}
                      disabled={starting || recording}
                      title={missing ? "This input does not exist on the selected interface" : undefined}
                      onChange={(e) =>
                        updateLanes(
                          lanes.map((x, k) =>
                            k === i ? { ...x, input: Number(e.target.value) } : x
                          )
                        )
                      }
                    >
                      {missing && <option value={l.input}>⚠ input {l.input}</option>}
                      {Array.from({ length: channels }, (_, n) => (
                        <option key={n + 1} value={n + 1}>
                          {String(n + 1).padStart(2, "0")} · {inputName(n + 1)}
                        </option>
                      ))}
                    </select>
                    <input
                      className="text-input lane-name"
                      data-lane-name={i}
                      value={l.name}
                      placeholder={inputName(l.input)}
                      title={
                        recording
                          ? "Rename while tracking — applied to the file when the take stops"
                          : "Layer name — becomes the file and layer name"
                      }
                      onChange={(e) =>
                        updateLanes(
                          lanes.map((x, k) =>
                            k === i ? { ...x, name: e.target.value } : x
                          )
                        )
                      }
                      onBlur={() => {
                        // Mid-take renames land in the backend, which
                        // renames the file at stop time.
                        if (recording) void api.recordRenameLane(i, l.name).catch(() => {});
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" && recording) {
                          void api.recordRenameLane(i, l.name).catch(() => {});
                          return;
                        }
                        if (e.key !== "Tab") return;
                        // Tab cycles through the layer names (wrapping
                        // at both ends), like track renaming.
                        const n = lanes.length;
                        const target = e.shiftKey ? (i - 1 + n) % n : (i + 1) % n;
                        const next = document.querySelector<HTMLInputElement>(
                          `[data-lane-name="${target}"]`
                        );
                        if (next) {
                          e.preventDefault();
                          next.focus();
                          next.select();
                        }
                      }}
                    />
                    <span className="record-lane-track">
                      <span
                        className={`record-lane-fill ${db > -3 ? "hot" : ""}`}
                        style={{ width: `${pct}%` }}
                      />
                    </span>
                    <button
                      className="btn btn-icon"
                      disabled={starting || recording}
                      title="Remove this lane"
                      onClick={() => updateLanes(lanes.filter((_, k) => k !== i))}
                    >
                      ✕
                    </button>
                  </div>
                );
              })}
            </div>
          )}
          {!recording && (
            <>
              <div className="lane-add">
                <span>Add</span>
                <input
                  className="text-input num-input"
                  type="number"
                  min={1}
                  max={Math.max(channels, 1)}
                  value={addCount}
                  disabled={starting}
                  onChange={(e) => setAddCount(e.target.value)}
                  onBlur={() => setAddCount(String(addCountN))}
                />
                <span>lane{addCountN > 1 ? "s" : ""} from</span>
                <select
                  className="select"
                  value={addFirst}
                  disabled={starting || channels === 0}
                  onChange={(e) => setAddFirst(Number(e.target.value))}
                >
                  {Array.from({ length: channels }, (_, n) => (
                    <option key={n + 1} value={n + 1}>
                      {String(n + 1).padStart(2, "0")} · {inputName(n + 1)}
                    </option>
                  ))}
                </select>
                <button className="btn" disabled={starting || channels === 0} onClick={addBatch}>
                  + Add
                </button>
              </div>
              <div className="hint">
                Consecutive inputs are assigned in order; adjust any lane's input or name after adding.
                Meters are live before recording — every line should move when you play.
              </div>
            </>
          )}
        </div>

        {!recording && (
          <div className="field">
            <label>Folder</label>
            <div className="dest-row">
              <input className="text-input mono" value={destDir} readOnly title={destDir} />
              <button
                className="btn"
                disabled={starting}
                onClick={async () => {
                  const dir = await openDialog({ directory: true, defaultPath: destDir || undefined });
                  if (typeof dir === "string") setDestDir(dir);
                }}
              >
                Choose…
              </button>
            </div>
            <div className="hint">
              A fresh "Take N" subfolder is created — existing files are never touched.
            </div>
          </div>
        )}
      </div>

      <div className={`record-clock-panel ${recording ? "rolling" : ""}`}>
        <div className="record-clock" title={recording ? status?.folder : undefined}>
          {recording && <span className="record-dot" />}
          {String(mm).padStart(2, "0")}:{String(ss).padStart(2, "0")}
        </div>
        <div className="record-clock-sub">
          {recording && status
            ? `${(status.sample_rate / 1000).toLocaleString("en-US", { maximumFractionDigits: 1 })} kHz · 24-bit — ${lanes.length} lane${lanes.length > 1 ? "s" : ""}`
            : monitoring && status
              ? `Ready — ${(status.sample_rate / 1000).toLocaleString("en-US", { maximumFractionDigits: 1 })} kHz · 24-bit · meters live`
              : lanes.length === 0
                ? "Add lanes to arm the machine"
                : "Waiting for the interface…"}
        </div>
        {!recording ? (
          <button
            className={`btn btn-primary record-btn record-big ${starting ? "arming" : ""}`}
            disabled={starting || lanes.length === 0 || invalidLanes || devices.length === 0}
            title={invalidLanes ? "Some lanes point at inputs the selected interface does not have" : undefined}
            onClick={() => void start()}
          >
            {starting ? "Arming…" : "● Record"}
          </button>
        ) : (
          <button className="btn btn-primary record-btn record-big recording" onClick={() => void stop()}>
            ■ Stop
          </button>
        )}
        {recording && status && (
          <div className="record-clock-folder mono" title={status.folder}>
            {status.folder}
          </div>
        )}
        {status && status.dropped_frames > 0 && (
          <div className="record-dropped">
            ⚠ {status.dropped_frames} frames dropped — the disk cannot keep up
          </div>
        )}
        {recording && !sawSignal.current && elapsed > 3 && (
          <div className="record-dropped">
            ⚠ No signal on any input yet —{" "}
            {micPerm === "denied" || micPerm === "restricted"
              ? "microphone access is blocked (see the interface panel)."
              : "check the selected interface, its inputs and the cables."}
          </div>
        )}
        {status?.error && <div className="record-dropped">⚠ {status.error}</div>}
      </div>
    </div>
  );
}

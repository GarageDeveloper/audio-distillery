import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import type { InputDeviceInfo } from "../types/InputDeviceInfo";
import type { RecordLane } from "../types/RecordLane";
import type { RecordStatus } from "../types/RecordStatus";
import { api } from "../api";
import { Backdrop } from "./Backdrop";

interface Props {
  onClose: () => void;
  onError: (msg: string) => void;
  /** Called with the recorded file paths after a successful stop. */
  onRecorded: (paths: string[]) => void;
}

const dbOf = (peak: number) =>
  peak > 0 ? Math.max(-60, 20 * Math.log10(peak)) : -60;

const SETUP_KEY = "still-record-setup";

/** Tape machine: pick an interface, build the lane list (any input, any
 * order, named at add time), then track. Display only — arming,
 * streaming and file writing are backend. */
export function RecordDialog({ onClose, onError, onRecorded }: Props) {
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

  const recording = status?.recording ?? false;
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
    // Adopt an already-running recording if the dialog was reopened.
    api.recordStatus().then((s) => s && setStatus(s)).catch(() => {});
    try {
      const saved = JSON.parse(localStorage.getItem(SETUP_KEY) ?? "null");
      if (saved?.lanes?.length) {
        setLanes(saved.lanes);
        if (saved.device) setDevice(saved.device);
      }
    } catch {
      /* fresh start */
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Hot-plug: the backend watcher (CoreAudio listener + off-thread
  // enumeration) pushes an event ONLY when the topology changes — no
  // polling, nothing on the UI thread.
  useEffect(() => {
    const un = listen<InputDeviceInfo[]>("record:devices", (e) => adoptDevices(e.payload));
    return () => {
      void un.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!recording) return;
    const t = window.setInterval(() => {
      api
        .recordStatus()
        .then((s) => {
          if (s) {
            s.levels.forEach((v, i) => {
              holds.current[i] = Math.max(v, (holds.current[i] ?? 0) * 0.82);
            });
            setStatus(s);
          }
        })
        .catch(() => {});
    }, 150);
    return () => window.clearInterval(t);
  }, [recording]);

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
    try {
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

  const elapsed = status?.elapsed_seconds ?? 0;
  const mm = Math.floor(elapsed / 60);
  const ss = Math.floor(elapsed % 60);

  return (
    <Backdrop onClose={() => {}}>
      <div className="modal record-modal">
        <div>
          <h2>Record</h2>
          <div className="subtitle">
            Tape machine: every lane goes straight to its own file, sample-synced.
          </div>
        </div>

        {!recording && (
          <>
            <div className="field">
              <label>Interface</label>
              <select
                className="select"
                value={device}
                disabled={starting}
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
            </div>

            <div className="field">
              <label>Lanes ({lanes.length})</label>
              {lanes.length > 0 && (
                <div className="lane-list">
                  {lanes.map((l, i) => {
                    const missing = channels > 0 && l.input > channels;
                    return (
                      <div key={i} className={`lane-row ${missing ? "missing" : ""}`}>
                        <span className="lane-num">{String(i + 1).padStart(2, "0")}</span>
                        <select
                          className="select lane-input"
                          value={l.input}
                          disabled={starting}
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
                          disabled={starting}
                          title="Layer name — becomes the file and layer name"
                          onChange={(e) =>
                            updateLanes(
                              lanes.map((x, k) =>
                                k === i ? { ...x, name: e.target.value } : x
                              )
                            )
                          }
                          onKeyDown={(e) => {
                            if (e.key !== "Tab") return;
                            // Tab cycles through the layer names (wrapping
                            // at both ends), like track renaming.
                            const n = lanes.length;
                            const target = e.shiftKey
                              ? (i - 1 + n) % n
                              : (i + 1) % n;
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
                        <button
                          className="btn btn-icon"
                          disabled={starting}
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
              </div>
            </div>

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
          </>
        )}

        {recording && status && (
          <>
            <div className="record-elapsed" title={status.folder}>
              <span className="record-dot" />
              {String(mm).padStart(2, "0")}:{String(ss).padStart(2, "0")}
              <span className="record-rate">
                {(status.sample_rate / 1000).toLocaleString("en-US", { maximumFractionDigits: 1 })} kHz · 24-bit
              </span>
            </div>
            <div className="record-meters">
              {status.levels.map((_, i) => {
                const lane = lanes[i];
                const hold = holds.current[i] ?? 0;
                const db = dbOf(hold);
                const pct = Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
                return (
                  <div key={i} className="record-lane" title={lane ? inputName(lane.input) : undefined}>
                    <span className="record-lane-num">
                      {lane?.name.trim() || (lane ? inputName(lane.input) : String(i + 1))}
                    </span>
                    <span className="record-lane-track">
                      <span
                        className={`record-lane-fill ${db > -3 ? "hot" : ""}`}
                        style={{ width: `${pct}%` }}
                      />
                    </span>
                  </div>
                );
              })}
            </div>
            {status.dropped_frames > 0 && (
              <div className="record-dropped">
                ⚠ {status.dropped_frames} frames dropped — the disk cannot keep up
              </div>
            )}
            {status.error && <div className="record-dropped">⚠ {status.error}</div>}
          </>
        )}

        <div className="modal-foot">
          {!recording ? (
            <>
              <button className="btn" disabled={starting} onClick={onClose}>
                Close
              </button>
              <button
                className={`btn btn-primary record-btn ${starting ? "arming" : ""}`}
                disabled={starting || lanes.length === 0 || invalidLanes || devices.length === 0}
                title={invalidLanes ? "Some lanes point at inputs the selected interface does not have" : undefined}
                onClick={() => void start()}
              >
                {starting ? "Arming…" : "● Record"}
              </button>
            </>
          ) : (
            <button className="btn btn-primary record-btn recording" onClick={() => void stop()}>
              ■ Stop
            </button>
          )}
        </div>
      </div>
    </Backdrop>
  );
}

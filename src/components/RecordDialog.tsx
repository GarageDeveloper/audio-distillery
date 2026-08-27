import { useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import type { InputDeviceInfo } from "../types/InputDeviceInfo";
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

/** Tape machine: pick an interface, a first input and a lane count, then
 * track. Display only — arming, streaming and file writing are backend. */
export function RecordDialog({ onClose, onError, onRecorded }: Props) {
  const [devices, setDevices] = useState<InputDeviceInfo[]>([]);
  const [device, setDevice] = useState<string>("");
  const [firstInput, setFirstInput] = useState(1);
  const [laneCount, setLaneCount] = useState(2);
  const [destDir, setDestDir] = useState("");
  const [status, setStatus] = useState<RecordStatus | null>(null);
  const [starting, setStarting] = useState(false);
  // Max-hold per lane so short peaks stay visible between polls.
  const holds = useRef<number[]>([]);

  const recording = status?.recording ?? false;
  const selected = devices.find((d) => d.name === device);
  const maxLanes = selected
    ? Math.max(1, selected.channels - firstInput + 1)
    : 16;

  useEffect(() => {
    api
      .listInputDevices()
      .then((list) => {
        setDevices(list);
        const def = list.find((d) => d.is_default) ?? list[0];
        if (def) {
          setDevice((cur) => (cur ? cur : def.name));
          setLaneCount((n) => Math.min(Math.max(n, 1), def.channels));
        }
      })
      .catch((e) => onError(String(e)));
    api
      .getDefaultRecordingDir()
      .then((d) => setDestDir((cur) => cur || d))
      .catch(() => {});
    // Adopt an already-running recording if the dialog was reopened.
    api.recordStatus().then((s) => s && setStatus(s)).catch(() => {});
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

  const start = async () => {
    setStarting(true);
    holds.current = [];
    try {
      const s = await api.recordStart({
        device,
        first_input: firstInput,
        layer_count: laneCount,
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
    <Backdrop
      onClose={() => {
        if (!recording) onClose();
      }}
    >
      <div className="modal record-modal">
        <div>
          <h2>Record</h2>
          <div className="subtitle">
            Tape machine: every armed input goes straight to its own file, sample-synced.
          </div>
        </div>

        {!recording && (
          <>
            <div className="field">
              <label>Interface</label>
              <select
                className="select"
                value={device}
                onChange={(e) => setDevice(e.target.value)}
              >
                {devices.length === 0 && <option value="">No input device found</option>}
                {devices.map((d) => (
                  <option key={d.name} value={d.name}>
                    {d.name} — {d.channels} input{d.channels > 1 ? "s" : ""} @{" "}
                    {(d.sample_rate / 1000).toLocaleString("en-US", {
                      maximumFractionDigits: 1,
                    })}{" "}
                    kHz
                  </option>
                ))}
              </select>
            </div>
            <div className="field-row">
              <div className="field">
                <label>First input</label>
                <input
                  className="text-input num-input"
                  type="number"
                  min={1}
                  max={selected?.channels ?? 64}
                  value={firstInput}
                  onChange={(e) =>
                    setFirstInput(Math.max(1, Number(e.target.value) || 1))
                  }
                />
              </div>
              <div className="field">
                <label>Inputs to record</label>
                <input
                  className="text-input num-input"
                  type="number"
                  min={1}
                  max={maxLanes}
                  value={laneCount}
                  onChange={(e) =>
                    setLaneCount(Math.max(1, Number(e.target.value) || 1))
                  }
                />
                <div className="hint">
                  Input {firstInput} → layer 1, input {firstInput + 1} → layer 2, …
                  {selected ? ` (device has ${selected.channels})` : ""}
                </div>
              </div>
            </div>
            <div className="field">
              <label>Folder</label>
              <div className="dest-row">
                <input className="text-input mono" value={destDir} readOnly title={destDir} />
                <button
                  className="btn"
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
                const hold = holds.current[i] ?? 0;
                const db = dbOf(hold);
                const pct = Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
                return (
                  <div key={i} className="record-lane" title={`input ${firstInput + i}`}>
                    <span className="record-lane-num">{String(firstInput + i).padStart(2, "0")}</span>
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
              <button className="btn" onClick={onClose}>
                Close
              </button>
              <button
                className="btn btn-primary record-btn"
                disabled={starting || devices.length === 0}
                onClick={() => void start()}
              >
                ● Record
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

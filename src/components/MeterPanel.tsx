import { useEffect, useRef, useState } from "react";
import type { MeterState } from "../types/MeterState";
import { api } from "../api";

interface Props {
  playing: boolean;
}

const TARGETS: { key: string; label: string; lufs: number | null }[] = [
  { key: "off", label: "No target", lufs: null },
  { key: "spotify", label: "Spotify −14", lufs: -14 },
  { key: "apple", label: "Apple Music −16", lufs: -16 },
  { key: "youtube", label: "YouTube −14", lufs: -14 },
  { key: "ebu", label: "EBU R128 −23", lufs: -23 },
];

const FLOOR = -40;

function barPct(v: number | null | undefined): number {
  if (v == null) return 0;
  return Math.max(0, Math.min(100, ((v - FLOOR) / -FLOOR) * 100));
}

function fmt(v: number | null | undefined, digits = 1): string {
  return v == null ? "—" : v.toFixed(digits);
}

/**
 * Master-bus loudness meters (EBU R128, post-chain): momentary/short-term
 * bars, integrated readout against an optional streaming target, LRA and
 * max-hold true peak. Display only — everything is measured backend-side.
 */
export function MeterPanel({ playing }: Props) {
  const [meter, setMeter] = useState<MeterState>({
    lufs_m: null,
    lufs_s: null,
    lufs_i: null,
    lra: null,
    true_peak_db: null,
  });
  const [target, setTarget] = useState(
    () => localStorage.getItem("still-meter-target") ?? "off"
  );
  const idle = useRef(0);

  useEffect(() => {
    localStorage.setItem("still-meter-target", target);
  }, [target]);

  useEffect(() => {
    // Fast while playing; slow trickle otherwise (values freeze anyway).
    const tick = () => {
      api.meterState().then(setMeter).catch(() => {});
    };
    tick();
    const t = setInterval(tick, playing ? 120 : 1000);
    idle.current = 0;
    return () => clearInterval(t);
  }, [playing]);

  const tgt = TARGETS.find((t) => t.key === target) ?? TARGETS[0];
  const delta = tgt.lufs != null && meter.lufs_i != null ? meter.lufs_i - tgt.lufs : null;
  const overTp = meter.true_peak_db != null && meter.true_peak_db > -1.0;

  return (
    <div className="meter-panel">
      <div className="meter-head">
        <span className="picker-section">Loudness</span>
        <button
          className="disc-sep-btn"
          title="Reset integrated loudness, LRA and the true-peak max hold"
          onClick={() => api.resetMeter().catch(() => {})}
        >
          ⟲
        </button>
      </div>
      {(["M", "S"] as const).map((k) => {
        const v = k === "M" ? meter.lufs_m : meter.lufs_s;
        return (
          <div key={k} className="meter-row" title={`LUFS-${k === "M" ? "M (momentary, 400 ms)" : "S (short-term, 3 s)"}`}>
            <span className="meter-tag">{k}</span>
            <div className="meter-track">
              <div className="meter-fill" style={{ width: `${barPct(v)}%` }} />
            </div>
            <span className="meter-val">{fmt(v)}</span>
          </div>
        );
      })}
      <div className="meter-readouts">
        <span className="meter-int" title="Integrated loudness (gated) since load/reset">
          <strong>{fmt(meter.lufs_i)}</strong> LUFS-I
          {delta != null && (
            <span className={`meter-delta ${Math.abs(delta) <= 1 ? "ok" : ""}`}>
              {delta >= 0 ? "+" : ""}
              {delta.toFixed(1)} LU
            </span>
          )}
        </span>
        <span title="Loudness range">LRA {fmt(meter.lra)}</span>
        <span className={overTp ? "meter-over" : ""} title="Max-hold true peak (dBTP); red above −1 dBTP">
          TP {fmt(meter.true_peak_db)}
        </span>
      </div>
      <select
        className="target-select meter-target"
        value={target}
        onChange={(e) => setTarget(e.target.value)}
        title="Loudness target — LUFS-I is compared against it"
      >
        {TARGETS.map((t) => (
          <option key={t.key} value={t.key}>
            {t.label}
          </option>
        ))}
      </select>
    </div>
  );
}

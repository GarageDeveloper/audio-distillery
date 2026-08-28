import { useEffect, useState } from "react";
import type { MeterState } from "../types/MeterState";
import { api } from "../api";

interface Props {
  playing: boolean;
  /** Clicking the rail opens the Master phase (full panel). */
  onOpen: () => void;
}

const FLOOR = -40;
const pct = (v: number | null | undefined) =>
  v == null ? 0 : Math.max(0, Math.min(100, ((v - FLOOR) / -FLOOR) * 100));

/** The mastering panel collapsed to its essentials for the Edit phase:
 * LUFS-I readout + M/S bars, always visible. Everything else waits in
 * the Master phase. */
export function MeterRail({ playing, onOpen }: Props) {
  const [meter, setMeter] = useState<MeterState>({
    lufs_m: null,
    lufs_s: null,
    lufs_i: null,
    lra: null,
    true_peak_db: null,
  });

  useEffect(() => {
    const tick = () => {
      api.meterState().then(setMeter).catch(() => {});
    };
    tick();
    const t = setInterval(tick, playing ? 120 : 1000);
    return () => clearInterval(t);
  }, [playing]);

  const overTp = meter.true_peak_db != null && meter.true_peak_db > -1;

  return (
    <button
      className="meter-rail"
      title="EBU R128 master meter — click to open the Master phase (chains, targets, true peak)"
      onClick={onOpen}
    >
      <span className={`meter-rail-int ${overTp ? "over" : ""}`}>
        {meter.lufs_i == null ? "—" : meter.lufs_i.toFixed(1)}
        <small>LUFS-I</small>
      </span>
      <span className="meter-rail-bar">
        <i style={{ height: `${pct(meter.lufs_m)}%` }} />
      </span>
      <span className="meter-rail-bar">
        <i style={{ height: `${pct(meter.lufs_s)}%` }} />
      </span>
      <span className="meter-rail-label">EBU R128</span>
    </button>
  );
}

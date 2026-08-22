import { useCallback, useEffect, useRef, useState } from "react";
import type { PlaybackState } from "../types/PlaybackState";
import { api } from "../api";

interface PlaybackDisplay {
  playing: boolean;
  positionSeconds: number;
  /** Output-device failure reported by the engine (playback is silent). */
  deviceError: string | null;
  /** Adopt a backend-returned state immediately (after toggle/seek). */
  adopt: (s: PlaybackState) => void;
}

/**
 * Polls the backend playback state and interpolates between polls so the
 * playhead moves smoothly. Display-only: the backend clock is authoritative.
 */
export function usePlayback(_sampleRate: number, active: boolean): PlaybackDisplay {
  const [playing, setPlaying] = useState(false);
  const [positionSeconds, setPositionSeconds] = useState(0);
  const [deviceError, setDeviceError] = useState<string | null>(null);
  const base = useRef({ pos: 0, at: performance.now(), playing: false });

  const adopt = useCallback((s: PlaybackState) => {
    base.current = { pos: s.position_seconds, at: performance.now(), playing: s.playing };
    setPlaying(s.playing);
    setPositionSeconds(s.position_seconds);
    setDeviceError(s.device_error);
  }, []);

  // Poll the authoritative state.
  useEffect(() => {
    if (!active) return;
    let stop = false;
    const tick = async () => {
      try {
        const s = await api.playerState();
        if (!stop) adopt(s);
      } catch {
        // Backend not ready yet; keep polling.
      }
    };
    void tick();
    const id = window.setInterval(tick, 250);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, [active, adopt]);

  // Smooth interpolation while playing.
  useEffect(() => {
    if (!active || !playing) return;
    const id = window.setInterval(() => {
      const b = base.current;
      if (b.playing) {
        setPositionSeconds(b.pos + (performance.now() - b.at) / 1000);
      }
    }, 50);
    return () => window.clearInterval(id);
  }, [active, playing]);

  return { playing, positionSeconds, deviceError, adopt };
}

import type { ProjectView } from "../types/ProjectView";

interface Props {
  view: ProjectView | null;
}

export function StatusBar({ view }: Props) {
  return (
    <div className="status-bar">
      <div className="hints">
        <span>
          <kbd>Space</kbd> Play/Pause
        </span>
        <span>
          <kbd>M</kbd> Track start, then end
        </span>
        <span>
          <kbd>Drag</kbd> Select region
        </span>
        <span>
          <kbd>⏎</kbd> Selection → track
        </span>
        <span>
          <kbd>←</kbd>
          <kbd>→</kbd> Seek
        </span>
        <span>
          <kbd>⌫</kbd> Delete track
        </span>
      </div>
      {view && (
        <span className="fileinfo">
          {view.audio.format} · {(view.audio.sample_rate / 1000).toLocaleString("en-US", { maximumFractionDigits: 1 })}{" "}
          kHz · {view.audio.channels === 1 ? "mono" : view.audio.channels === 2 ? "stereo" : `${view.audio.channels} ch`}
        </span>
      )}
    </div>
  );
}

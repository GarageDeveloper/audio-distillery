interface Props {
  onOpen: () => void;
}

export function EmptyState({ onOpen }: Props) {
  return (
    <div className="empty-state">
      <svg className="empty-wave" width="320" height="64" viewBox="0 0 320 64">
        {Array.from({ length: 40 }, (_, i) => {
          const h = 6 + 26 * Math.abs(Math.sin(i * 0.55) * Math.sin(i * 0.13));
          return (
            <line
              key={i}
              x1={i * 8 + 4}
              x2={i * 8 + 4}
              y1={32 - h}
              y2={32 + h}
              stroke="currentColor"
              strokeWidth="2"
              strokeDasharray="3 3"
            />
          );
        })}
      </svg>
      <h1>Drop your audio files here</h1>
      <div className="sub">or press Open — WAV, FLAC, MP3, AIFF · or a .still project</div>
      <button className="btn btn-primary" onClick={onOpen}>
        Open a file…
      </button>
    </div>
  );
}

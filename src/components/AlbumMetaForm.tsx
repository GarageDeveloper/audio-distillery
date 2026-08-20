import type { AlbumMeta } from "../types/AlbumMeta";

interface Props {
  meta: AlbumMeta;
  onChange: (meta: AlbumMeta) => void;
  /** Show the disc-breaks text field (the track list also manages breaks). */
  showDiscBreaks?: boolean;
}

/** Shared album-metadata form: used by the Album dialog (main UI) and the
 * export dialog. Values are the backend's; every edit is an intention. */
export function AlbumMetaForm({ meta, onChange, showDiscBreaks }: Props) {
  const set = (patch: Partial<AlbumMeta>) => onChange({ ...meta, ...patch });
  return (
    <div className="meta-grid">
      <div className="field">
        <label>Album</label>
        <input
          className="text-input"
          value={meta.album}
          placeholder="Live at the Barn"
          onChange={(e) => set({ album: e.target.value })}
        />
      </div>
      <div className="field">
        <label>Album artist</label>
        <input
          className="text-input"
          value={meta.album_artist}
          placeholder="The Copper Stills"
          onChange={(e) => set({ album_artist: e.target.value })}
        />
      </div>
      <div className="field">
        <label>Track artist</label>
        <input
          className="text-input"
          value={meta.artist}
          placeholder="empty = album artist"
          onChange={(e) => set({ artist: e.target.value })}
        />
      </div>
      <div className="field">
        <label>Date</label>
        <input
          className="text-input"
          value={meta.date}
          placeholder="2026-08-01"
          onChange={(e) => set({ date: e.target.value })}
        />
      </div>
      <div className="field">
        <label>Genre</label>
        <input
          className="text-input"
          value={meta.genre}
          placeholder="Rock"
          onChange={(e) => set({ genre: e.target.value })}
        />
      </div>
      {showDiscBreaks && (
        <div className="field">
          <label>Disc breaks</label>
          <input
            className="text-input mono"
            value={meta.disc_breaks.join(", ")}
            placeholder="e.g. 7, 13"
            title="Track numbers starting a new disc — also editable directly in the track list"
            onChange={(e) =>
              set({
                disc_breaks: e.target.value
                  .split(/[\s,;]+/)
                  .map((v) => parseInt(v, 10))
                  .filter((n) => Number.isFinite(n) && n > 0),
              })
            }
          />
        </div>
      )}
      <div className="field meta-comment">
        <label>Comment</label>
        <input
          className="text-input"
          value={meta.comment}
          onChange={(e) => set({ comment: e.target.value })}
        />
      </div>
      <div className="hint meta-hint">
        Track n°/total and disc n°/total are computed automatically (add disc breaks in
        the track list). Every field accepts macros:{" "}
        {"{title} {n} {ntotal} {disc} {dtotal} {album} {artist} {date} {year} {source}"} —
        e.g. Album = {'"Anthology (Disc {disc})"'}
      </div>
    </div>
  );
}

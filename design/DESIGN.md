# AudioDistillery ("Still") — Art direction & design system

> Design-team deliverable — accompanies the mockups `direction-a.html`,
> `direction-b.html`, `direction-c.html` (open them in a browser; each page
> shows the main screen in editing state, the export dialog in settings,
> the progress variant and the final report).

---

## 1. The three directions

### Direction A — "Alambic" (`direction-a.html`)

**Thesis.** Still means *alembic* — a copper still. The still's material is
copper. This direction therefore embraces a **warm** palette — smoky
brown-black and glowing copper — where 100 % of the audio tools on the
market are cold (grey/blue). The waveform is treated as molten material: a
vertical copper gradient (dark at the peaks, glowing at the center),
doubled by a brighter RMS layer over the peak layer.

- Signature: copper waveform + markers as "cask labels" (dovetail flags
  M1…M5).
- Tone: analog hardware, VU meters, tape machines. Tabular monospace
  digits, engraved rules.
- Accepted risk: a chromatic identity unique in the category, derived
  directly from the product name.

### Direction B — "Signal" (`direction-b.html`)

**Thesis.** Spectral precision in the FabFilter / iZotope RX vein. Deep
blue-black, lightly glassy panels, and a functional signature: the **two
channels color-coded** (ice cyan = left, indigo = right) with an L/R
legend in the toolbar. Luminescent playhead (white glow), minimap as a
preview strip above the waveform, timecode in a dedicated dial.

- Signature: L/R bi-coloring — carrying real information (channel
  imbalance visible at a glance).
- Tone: measuring instrument, oscilloscope. The most "pro-audio" of the
  three.
- Limit: also the most expected look of the category — competent but not
  differentiating.

### Direction C — "Atelier" (`direction-c.html`)

**Thesis.** Radical calm, Ableton spirit: warm graphite (lighter than the
usual backgrounds), flat surfaces without relief or gradients,
**monochrome ivory waveform**. Color never decorates: bottle green is
reserved for what can be manipulated (markers, play button, actions).
Generous radii, large targets, relaxed density.

- Signature: the chromatic discipline itself — a single green, purely
  functional.
- Tone: a simple, friendly tool, true to the product's "10× simpler"
  brief.
- Limit: a soft identity; the product risks looking like "one more
  utility app" rather than a desirable audio tool.

---

## 2. Recommendation: Direction A — "Alambic"

**Why A.**

1. **Unique brand anchor.** The copper palette flows literally from the
   name (Still = alembic = copper). No competitor can claim it; B could
   be any plugin, C any utility.
2. **Waveform legibility.** Copper on brown-black gives excellent
   contrast without harshness, and the peak (dark) / RMS (glowing) pair
   makes the recording's dynamics immediately readable — the heart of the
   app's job.
3. **Real-world comfort.** You split a concert at night, on headphones: a
   warm dominant tires less than saturated cyan, and stays credibly "pro"
   through typographic rigor (tabular monospace, rules, controlled
   density).
4. **Compatible with the intended simplicity.** A borrows C's discipline
   (a single accent color, sober hierarchy) while keeping a strong
   identity. We also recommend **borrowing C's generous targets** (28 px
   marker handles, slightly softer radii) — integrated in the specs
   below.

**What we keep from the other directions.** From B: the time tooltip
during marker drags and the contrasted minimap. From C: the large click
targets and the direct writing tone.

**Point of vigilance.** With a single warm accent, semantic states must
stay discernible: the success green and error red defined below are
deliberately desaturated to coexist with the copper without competing.

---

## 3. Design system — Direction A "Alambic"

### 3.1 Full CSS tokens

```css
/* ==========================================================
   Still — Design tokens. Dark theme by default.
   Switch to light via [data-theme="light"] on <html>.
   ========================================================== */
:root {
  /* --- Backgrounds --- */
  --bg:            #17130E;  /* app background (smoky brown-black) */
  --bg-deep:       #100D09;  /* waveform area, minimap, fields */
  --panel:         #1F1A13;  /* track panel, modals */
  --panel-2:       #262018;  /* buttons, surface hover */
  --raised:        #2B241B;  /* button hover, grabbed elements */

  /* --- Lines --- */
  --line:          #322A20;  /* structural borders */
  --line-soft:     #262017;  /* time grid, weak separators */

  /* --- Text --- */
  --text:          #EFE6D8;  /* primary */
  --text-2:        #B3A48D;  /* secondary (labels, headers) */
  --text-3:        #7D7060;  /* tertiary (meta, hints, status bar) */
  --text-on-accent:#1C1207;  /* text on copper backgrounds */

  /* --- Copper accent --- */
  --copper:        #E0883A;  /* main accent */
  --copper-hi:     #F4B269;  /* hover, active values, timecode */
  --copper-lo:     #A85E24;  /* active borders, pressed */
  --copper-dim:    rgba(224, 136, 58, .14);  /* selected backgrounds */

  /* --- Waveform --- */
  --wave-l-peak:   #6E4A22;  /* left channel, peak layer (dark) */
  --wave-l-rms:    #F4B269;  /* left channel, RMS layer (copper gradient) */
  --wave-r-peak:   #5A3C1D;  /* right channel, peak layer */
  --wave-r-rms:    #D89050;  /* right channel, RMS — one notch softer */
  --wave-bg:       #100D09;  /* waveform area background */
  --wave-center:   #2B2318;  /* each channel's center line */
  --grid-time:     #262017;  /* time grid verticals */
  --minimap-wave:  #A87B45;  /* minimap's mono waveform */

  /* --- Cursors & selection --- */
  --playhead:      #F8EFE2;  /* playhead (warm white) */
  --playhead-glow: rgba(248, 239, 226, .65);
  --marker:        #E0883A;  /* marker line */
  --marker-glow:   rgba(224, 136, 58, .50);
  --marker-flag-a: #E89043;  /* flag gradient (top) */
  --marker-flag-b: #C06D28;  /* flag gradient (bottom) */
  --selection:     rgba(244, 178, 105, .10); /* current segment, minimap range */

  /* --- Semantics --- */
  --ok:            #8FBF6E;
  --ok-dim:        rgba(143, 191, 110, .10);
  --err:           #E0584A;
  --err-dim:       rgba(224, 88, 74, .10);
  --warn:          #D9A441;

  /* --- Focus --- */
  --focus-ring:    #F4B269;

  /* --- Geometry --- */
  --r-s: 4px;   /* fields, kbd, small elements */
  --r-m: 7px;   /* buttons, list rows, selects */
  --r-l: 11px;  /* modals, cards */
  --r-round: 999px; /* chips, play button */

  /* --- Shadows --- */
  --shadow-pop:   0 8px 24px rgba(0,0,0,.45);                       /* menus, tooltips */
  --shadow-modal: 0 24px 64px rgba(0,0,0,.55), 0 2px 8px rgba(0,0,0,.40);

  /* --- Type --- */
  --font-ui:   -apple-system, BlinkMacSystemFont, "Segoe UI Variable Text",
               "Segoe UI", system-ui, "Noto Sans", sans-serif;
  --font-mono: ui-monospace, "SF Mono", "Cascadia Mono", "JetBrains Mono",
               Menlo, Consolas, monospace;

  /* --- Motion --- */
  --ease: cubic-bezier(.25,.1,.25,1);
  --t-fast: 120ms;  /* hover, pressed */
  --t-med:  200ms;  /* panel opening, modal */
}

/* ---------- Light variant (optional) ---------- */
[data-theme="light"] {
  --bg:            #F4EFE6;
  --bg-deep:       #EAE3D6;
  --panel:         #FBF8F2;
  --panel-2:       #F0EADF;
  --raised:        #E6DECF;

  --line:          #D8CDBB;
  --line-soft:     #E4DBCB;

  --text:          #2B2115;
  --text-2:        #6B5A43;
  --text-3:        #9A8C77;
  --text-on-accent:#FFF6EA;

  --copper:        #C06818;
  --copper-hi:     #A85E24;   /* in light mode, hover darkens */
  --copper-lo:     #8A4C15;
  --copper-dim:    rgba(192, 104, 24, .12);

  --wave-l-peak:   #D9B58A;
  --wave-l-rms:    #B4661C;
  --wave-r-peak:   #E2C6A4;
  --wave-r-rms:    #C97F35;
  --wave-bg:       #EAE3D6;
  --wave-center:   #D8CDBB;
  --grid-time:     #E0D6C4;
  --minimap-wave:  #B08A5C;

  --playhead:      #2B2115;
  --playhead-glow: rgba(43, 33, 21, .35);
  --marker:        #C06818;
  --marker-glow:   rgba(192, 104, 24, .35);
  --marker-flag-a: #CE7420;
  --marker-flag-b: #A85E24;
  --selection:     rgba(192, 104, 24, .10);

  --ok:            #4E8A34;
  --ok-dim:        rgba(78, 138, 52, .12);
  --err:           #C23B2E;
  --err-dim:       rgba(194, 59, 46, .10);
  --warn:          #A87414;

  --focus-ring:    #C06818;
  --shadow-pop:    0 8px 24px rgba(60, 44, 20, .18);
  --shadow-modal:  0 24px 64px rgba(60, 44, 20, .25), 0 2px 8px rgba(60,44,20,.12);
}
```

### 3.2 Typography

No downloaded fonts: system stack only.

| Role | Font | Size | Weight | Notes |
|---|---|---|---|---|
| UI body (buttons, lists, labels) | `--font-ui` | 13 px | 400–500 | line-height 1.45 |
| Track name (list) | `--font-ui` | 13 px | 500 | ellipsis when too long |
| Modal titles | `--font-ui` | 16 px | 600 | |
| "STILL" wordmark | `--font-ui` | 13 px | 600 | `letter-spacing:.30em`, all-small-caps |
| Section headers (TRACKS, FORMAT…) | `--font-ui` | 11 px | 700 | uppercase, `letter-spacing:.12–.14em`, color `--text-2` |
| Main timecode | `--font-mono` | 15 px | 500 | `font-variant-numeric: tabular-nums`, color `--copper-hi` |
| Durations, meta, time ruler | `--font-mono` | 10–12 px | 400 | tabular-nums everywhere digits move |
| Hints / status bar | `--font-ui` | 11 px | 400 | color `--text-3` |
| `kbd` | `--font-mono` | 10 px | 400 | background `--panel-2`, border `--line`, 2 px bottom edge |

**Golden rule**: any number that can scroll (timecode, durations,
percentages) is tabular monospace — zero layout jitter during playback.

### 3.3 Spacing, radii, shadows

- Spacing scale: **4 / 8 / 12 / 16 / 20 / 24 px** (base 4). Standard
  container padding: 12–16 px; list gutters: 6–8 px.
- Radii: `--r-s` 4 px (fields), `--r-m` 7 px (buttons, rows), `--r-l`
  11 px (modals), `--r-round` (chips, play).
- Shadows: only `--shadow-pop` (tooltips, menus) and `--shadow-modal`
  (modals). **No shadow on embedded surfaces** — depth comes from the
  background steps (`--bg-deep` < `--bg` < `--panel` < `--panel-2` <
  `--raised`).
- Borders: 1 px `--line` for structure; hover lightens the background
  before lightening the border.

### 3.4 Zone specification

Default window 1360×860 (min 1024×640). Vertical order: toolbar → ruler →
**waveform** → minimap → status bar; track panel on the right.

#### Toolbar — h 52 px, gradient background `#211B13 → #1C1710`, bottom border `--line`
- Left: wordmark + copper drop (18 px), separator, **Open** button (30 px
  tall), then file name (600) + mono meta (`1:12:36 · 44.1 kHz · stereo`,
  `--text-3`).
- Center: transport — previous / **round 34 px play** (copper icon,
  border `#4A3B26`) / next — then timecode `27:35 / 1:12:36`.
- Right: primary **Export…** button (copper gradient, `--text-on-accent`
  text), panel toggle (icon, `aria-pressed` state).
- Buttons: h 30 px, 12 px padding, `--r-m` radius; 12–14 px icons,
  1.4–1.6 stroke.

#### Time ruler — h 26 px, `--bg-deep` background
Major ticks every 10 min (mono 10 px labels, `--text-3`), minor every
5 min; density adapts to zoom (target: one label at least every ~110 px).

#### Waveform area — flexible (≈ 620 px at 860 tall), `--wave-bg` background with a light radial vignette
- Two channels: centers at 27 % and 73 % of the height, max amplitude
  21 %; 1 px `--wave-center` center line per channel.
- Per-channel rendering: a **peak** layer (2 px bars, `--wave-*-peak`)
  under an **RMS** layer (≈ 45 % of the peak height, vertical copper
  gradient `--copper-lo → --copper-hi → --copper-lo`). Right channel one
  notch softer than the left so stereo reads without a legend.
- **Segments**: between two markers, a centered chip at the top (top
  12 px) — number alone (`1`), or `3 · Ashes` for the playing segment
  (solid copper chip, `--text-on-accent` text). Current segment
  background: `--selection`.
- **Markers**: 1 px `--marker` line + glow; 20 px dovetail flag (`M1`…
  mono 10 px 700). **Grab target: 28 px wide** (invisible zone centered
  on the line), `ew-resize` cursor. Hover: 3 px line. Drag: 3 px line +
  stronger glow + mono tooltip `34:10.2` under the flag.
- **Playhead**: 1 px `--playhead` + glow + 12 px downward triangle at the
  top. Always above the markers (z-order: segments < grid < wave <
  markers < playhead).

#### Minimap — h 64 px, below the waveform, `--bg-deep` background, top border `--line`
- Mono waveform (`--minimap-wave`, opacity .75), markers as 1 px copper
  55 % strokes, playhead 1 px warm white.
- **Viewport rectangle**: 1 px `--copper` border, `--copper-dim` fill,
  3 px radius, `grab`/`grabbing` cursor. Click outside the viewport =
  recenter; drag = pan; wheel over the waveform = zoom (the rectangle
  shrinks).

#### Track panel — w 284 px, collapsible (`--t-med` translation), `--panel` background, left border `--line`
- Header h 40 px: `TRACKS` (11 px 700 uppercase) + `6 · 1:12:36` (mono,
  `--text-3`).
- Row: `26px 1fr auto` grid, h ≈ 38 px, `--r-m` radius — number (mono,
  right-aligned), name (500, ellipsis), duration (mono 11 px).
  - hover → `--panel-2` background; playing track → `--copper-dim`
    background + `rgba(224,136,58,.28)` border + copper 700 number;
  - editing → full-width input, `--bg-deep` background, `--copper-lo`
    border.
- Footer h ≈ 36 px: `Double-click to rename` hint + `6 tracks`.
- List selection ⇄ segment highlight on the waveform (bidirectional
  coupling).

#### Status bar — h 28 px, `--bg-deep` background, top border `--line`
- Left: shortcuts as `kbd` + labels: `Space Play/Pause · M Add marker ·
  ← → Seek · ⌫ Delete marker`.
- Right: file format in mono: `WAV · 44.1 kHz · 16-bit · stereo`. The
  status bar also hosts transient messages ("Marker added at 27:35",
  3 s).

#### Export modal — w 480 px, centered, `--r-l` radius, `--shadow-modal`, backdrop `rgba(10,8,5,.62)` + 2 px blur
1. **Settings**: title `Export 6 tracks` + file subtitle; **Format** as a
   segmented control (WAV / FLAC / MP3 / AAC — active segment in copper
   gradient); **Quality** (visible for MP3/AAC: 320/256/192/V0) and
   **Sample rate** on one row; **Destination** (mono path + `Choose…`);
   **File naming** (mono input `{n} - {title}`, hint:
   `Preview: 03 - Ashes.mp3 — placeholders: {n} {title} {date}`). Footer:
   `Cancel` / `Export 6 tracks` (primary).
2. **Progress**: 6-row list (`n° · name · 4 px bar · mono status` —
   `done` green, `62%` copper on `--copper-dim`, `waiting` grey); 7 px
   global bar (copper gradient); `Track 4 of 6` / `58% · ~1 min left`;
   `Cancel export` button.
3. **Report**: success banner (`--ok-dim` + round check) `6 tracks
   exported` + path + total size; mono file list
   (`✓ 01 - Intro & Tuning.mp3`…); footer `Show in Finder` ("Show in
   Explorer" on Windows, "Show in Files" on Linux) / `Done` (primary).
4. **Error** (per track): the row turns `--err-dim`, `failed` status in
   `--err`; the report lists `✗ 04 - Boreal.mp3 — destination is not
   writable` with a `Retry failed` button. Other tracks keep exporting.

#### Empty state (no file)
- Reduced toolbar (Open alone active, transport and Export disabled at
  40 % opacity), panel and minimap hidden.
- Centered in the waveform area: a dotted copper waveform silhouette at
  20 %, title `Drop an audio file here` (16 px 600), subline `or press
  Open — WAV, FLAC, MP3, AAC · up to 4 hours` (`--text-3`), `Open a
  file…` button.
- Drag-over: inner 2 px `--copper` animated dotted border (disabled with
  `prefers-reduced-motion`), `--copper-dim` background.

#### Loading / analysis state
- The file name appears immediately in the toolbar.
- Waveform area: the waveform **draws itself left to right** as analysis
  progresses (analyzed columns appear in copper, the rest in
  `--line-soft`) — the progress bar is the waveform itself; a discreet
  centered label `Analyzing… 42%` (mono).
- Cancellable: `Esc` or a `Cancel` button under the label.

### 3.5 Full UX flow

1. **Open** — Drop or `Open` → analysis with waveform-as-progress → the
   app lands in editing: a single "Track 01", playhead at 0, minimap
   visible, panel open.
2. **Listen / navigate** — `Space` play-pause; click on the waveform or
   minimap = seek; `← →` ±5 s, `Shift+← →` ±30 s; wheel = zoom around the
   cursor; `↑ ↓` previous/next track (seek to track start).
3. **Mark** — `M` drops a marker at the playhead (or double-click the
   waveform at the desired spot). Track numbers recompute instantly; the
   new track appears in the panel with a default name `Track 04`.
   Dragging a flag = fine adjustment (time tooltip, light auto-zoom at
   the edges). `⌫` on a selected marker deletes it (the two tracks merge,
   the first name survives; undoable).
4. **Name** — Double-click the name in the panel (or `Enter` on the
   selected row) → inline input, text preselected; `Enter` confirms,
   `Esc` cancels, `Tab` confirms and moves to the next track (the "I name
   the whole set" flow without touching the mouse).
5. **Export** — `Export…` → settings modal (remembers last choices) →
   per-track + global progress → final report with `Show in Finder`. The
   app stays usable during export (the modal can be reduced to a progress
   pill in the toolbar).
6. **Global undo/redo** — `Cmd/Ctrl+Z` covers marker add/move/delete and
   renames.

### 3.6 Micro-interactions

| Interaction | Behaviour |
|---|---|
| Marker hover | Line 1→3 px, flag lightens (`--t-fast`), `ew-resize` cursor |
| Marker drag | Stronger glow, mono time tooltip under the flag, optional snap to detected silences (±250 ms magnet, disable with `Alt`), panel durations update live |
| Marker drop (`M`) | The flag "falls" 6 px with a 150 ms fade (no animation with `prefers-reduced-motion`); status-bar message `Marker added at 27:35` |
| Inline rename | Copper border + preselected text; confirmation = 300 ms `--copper-dim` flash on the row |
| Play/pause | The icon flips without animation; the copper timecode is the only thing permanently "moving" |
| Seek (click) | The playhead jumps without transition (precision first), light triangle flash |
| Zoom | Centered on the mouse; the minimap rectangle animates in `--t-fast` |
| Panel toggle | 200 ms `--ease` translation; the waveform resizes continuously |
| Export started | The `Export…` button becomes a progress pill `Exporting 58%` when the modal is reduced |
| Errors | Never an alert modal for benign cases: inline banner in the export modal or `--err`-tinted status-bar message. E.g. `Can't read this file — format not supported (.aiff coming soon)` |
| Keyboard focus | `outline: 2px solid var(--focus-ring); outline-offset: 2px` on every interactive element |

### 3.7 Accessibility & guardrails

- Contrast: `--text` on `--bg` ≈ 13:1; `--text-2` ≈ 7:1; `--text-3`
  reserved for non-essential meta. `--text-on-accent` on copper ≥ 7:1.
- `prefers-reduced-motion`: all decorative animations off (progress bars
  stay).
- Every toolbar command is keyboard-accessible; the track list is a real
  navigable list (`↑ ↓`, `Enter` to rename).
- Glows (playhead, markers) are decorative: the information stays
  readable without them (solid lines).

---

## 4. Files

- `design/direction-a.html` — Direction A "Alambic" (recommended)
- `design/direction-b.html` — Direction B "Signal"
- `design/direction-c.html` — Direction C "Atelier"
- `design/DESIGN.md` — this document

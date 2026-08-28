# Workflow phases — proposal (for discussion, nothing implemented)

The app grew from "split one recording" to record → edit → master →
deliver, and every control now sits at the same level: a first-time user
faces the mastering panel before they have a single track, and the
export dialog carries eleven concerns. This document proposes how (and
how much) to organize the UI around the three natural phases of a
session, and what each option costs. **Decision wanted before any code.**

## The three phases, mapped onto what already exists

Every phase boundary already exists in the code as a state test — no new
persisted state would be needed (the frontend stays a display terminal;
"phase" is *derived*, never stored):

| Phase | What it covers today | Existing gate |
|---|---|---|
| **1 · Record / Import** | EmptyState, ● Rec dialog, drop-choice modal (one-after-another / as album tracks / multitrack / layers / takes) | `view == null`, or importing into an open session |
| **2 · Edit** | Waveform + Minimap, clips (⋯ menu), selection/markers, auto-split review, TrackList, per-track mixes | `view != null`; track tooling meaningful once `tracks.length > 0` |
| **3 · Master / Deliver** | MasteringPanel (chains + meter), AlbumStrip (target timeline), Album dialog (metadata, gaps, disc breaks), Export dialog (formats, stems, CD/DDP, ISRC) | `tracks.length > 0` is when all of it becomes actionable |

Observation from the M2 work: the **AlbumStrip is the natural bridge**
between phases 2 and 3 — in Edit it is the live *result preview*; in
Master it is the primary surface (the thing you listen to and space out).

## Option A — phase emphasis (recommended)

A three-segment switcher in the toolbar: `Record · Edit · Master`.
Switching **emphasizes**, never walls off:

- **Record**: the record dialog becomes a full-pane surface (lanes,
  meters, elapsed) instead of a modal; the timeline stays visible below,
  dimmed. Import actions (Open, +Clip, +Take) live here.
- **Edit**: today's center view, with the MasteringPanel *collapsed to a
  meter-only rail* (the EBU meter stays — you always want it) and the
  AlbumStrip present as preview.
- **Master**: the AlbumStrip doubles in height and becomes the primary
  transport; MasteringPanel fully open; TrackList shows the gap/ISRC/
  disc-break affordances prominently; Export is the phase's main CTA.

Soft gating rules:
- The switcher never blocks: all data stays reachable in every phase
  (a shortcut or click into a hidden panel simply switches phase).
- The app *suggests* the phase on state changes (fresh recording lands
  in Edit — this already happens; first track created pulses the Master
  segment once) but never switches on its own after the user has chosen.
- Keyboard: `1/2/3` or `⌘⇧←/→` to switch; transport keys identical in
  every phase (Space/M/arrows never change meaning).

Cost: medium — mostly conditional rendering over existing gates, one new
full-pane record surface, a collapsed-rail variant of MasteringPanel.
Risk: low (no data-model change, no new commands).

## Option B — status quo plus progressive disclosure

No switcher. Keep one surface but let it breathe:
- MasteringPanel starts collapsed until the first track exists.
- Export button gains a small "album readiness" checklist popover
  (tracks ✓, titles ✓, gaps ✓, metadata ✓, chain ✓).
- Record stays a modal.

Cost: small. Risk: none — but it does not answer the core complaint
(everything at the same level), it only trims the worst moments.

## Option C — wizard-style hard gating (rejected)

Sequential screens with Next/Back. Rejected because the real workflow is
non-linear: you re-record a take mid-edit (the record-into-session flow
cuts across phases by design), tweak a chain while marking, and export
partial bounces to check levels. Walls would fight the user within the
first hour.

## Trade-offs to settle

1. **Discoverability vs ceremony** — A makes each phase legible at the
   cost of one more global control; B keeps zero ceremony but keeps the
   flat hierarchy.
2. **Where the meter lives** — proposal: the EBU meter is phase-less
   (always visible, even in Record where it would show input peaks
   later); only the *chain editing* UI is phase-emphasized.
3. **Record as pane vs modal** — a pane makes long tracking sessions
   livable (you see the timeline grow if we later stream takes in);
   the modal is fine for short takes. A can ship with the modal first.
4. **What the first launch shows** — A: the Record segment with the
   EmptyState inside it (today's screen, relabeled); nothing else
   changes for the first-run experience.

## Mock-up scope (next step if Option A is chosen)

Three annotated wireframes (one per phase) plus the state matrix:

```
state \ phase        Record          Edit            Master
no session           EmptyState+Rec  (redirects R)   (redirects R)
session, 0 tracks    Rec-into-sess   full edit       meter rail only
tracks, no chain     idem            full edit       strip + chains empty
ready to export      idem            full edit       strip + chains + export CTA
```

Wireframe sketch of Option A's Master phase (target-timeline-first):

```
┌ Toolbar ─ [Record | Edit | ●Master] ──────────────────────────────┐
│ ┌ AlbumStrip (tall: blocks + gaps + transport + per-track meta) ┐ │
│ ├ Waveform (reduced height, read-mostly, playhead ghosted) ─────┤ │
│ ├ TrackList (gaps/ISRC/disc-breaks columns visible) ──┬─ Chains ┤ │
│ └──────────────────────────────────────────────────────┴─ Meter ┘ │
│ [ Album metadata… ]                        [ Export album… ]      │
└───────────────────────────────────────────────────────────────────┘
```

## Decision

**Option A adopted** (2026-08-28). Visual previews of the three phases
live in [`design/mockups/workflows-a.html`](mockups/workflows-a.html) —
to be reviewed before any implementation begins.

## Recommendation

Option **A**, shipped in two steps: (1) the switcher + emphasis rules
over existing components (no new surfaces), (2) the full-pane Record
surface later, once input metering during tracking (#8 follow-ups)
justifies it. Option B's checklist popover is worth keeping as part of
A's Master phase regardless.

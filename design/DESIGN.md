# AudioDistillery (« Still ») — Direction artistique & système de design

> Livrable de l'équipe design — accompagne les maquettes `direction-a.html`, `direction-b.html`, `direction-c.html` (à ouvrir dans un navigateur ; chaque page montre l'écran principal en état édition, le dialogue d'export en réglages, la variante progression et le rapport final).
>
> **Note produit** : la langue par défaut de l'interface est l'**anglais** (localisation ultérieure). Toutes les chaînes UI de ce document et des maquettes sont donc en anglais ; ce document reste en français.

---

## 1. Les trois directions

### Direction A — « Alambic » (`direction-a.html`)

**Thèse.** Still signifie *alambic*. Le matériau de l'alambic, c'est le cuivre. La direction assume donc une palette **chaude** — brun-noir fumé et cuivre incandescent — là où 100 % des outils audio du marché sont froids (gris/bleu). La waveform est traitée comme de la matière en fusion : dégradé cuivre vertical (sombre aux crêtes, incandescent au centre), doublé d'une couche RMS plus brillante par-dessus la couche peak.

- Signature : waveform cuivre + marqueurs en « étiquettes de fût » (fanions à queue d'aronde M1…M5).
- Ton : hardware analogique, VU-mètres, machines à bande. Chiffres monospace tabulaires, filets gravés.
- Risque assumé : une identité chromatique unique dans la catégorie, directement dérivée du nom du produit.

### Direction B — « Signal » (`direction-b.html`)

**Thèse.** Précision spectrale façon FabFilter / iZotope RX. Bleu-noir profond, panneaux en léger verre, et une signature fonctionnelle : les **deux canaux codés en couleur** (cyan glace = gauche, indigo = droit) avec légende L/R dans la toolbar. Playhead luminescent (glow blanc), minimap en bandeau d'aperçu au-dessus de la waveform, timecode dans un cadran dédié.

- Signature : bicolorisation L/R — porteuse d'information réelle (déséquilibre de canaux visible d'un coup d'œil).
- Ton : instrument de mesure, oscilloscope. Le plus « pro-audio » des trois.
- Limite : c'est aussi le look le plus attendu de la catégorie — compétent mais peu différenciant.

### Direction C — « Atelier » (`direction-c.html`)

**Thèse.** Calme radical, esprit Ableton : graphite chaud (plus clair que les fonds habituels), aplats sans relief ni dégradé, **waveform monochrome ivoire**. La couleur ne décore jamais : le vert bouteille est réservé à ce qui se manipule (marqueurs, bouton play, actions). Rayons généreux, cibles larges, densité détendue.

- Signature : la discipline chromatique elle-même — un seul vert, uniquement fonctionnel.
- Ton : outil simple et amical, fidèle au « 10× plus simple » du cahier des charges.
- Limite : identité douce ; le produit risque de ressembler à « une app utilitaire de plus » plutôt qu'à un outil audio désirable.

---

## 2. Recommandation : Direction A — « Alambic »

**Pourquoi A.**

1. **Ancrage de marque unique.** La palette cuivre découle littéralement du nom (Still = alambic = cuivre). Aucun concurrent ne peut la revendiquer ; B pourrait être n'importe quel plugin, C n'importe quel utilitaire.
2. **Lisibilité de la waveform.** Le cuivre sur brun-noir offre un excellent contraste sans agressivité, et le couple peak (sombre) / RMS (incandescent) rend la dynamique du concert immédiatement lisible — c'est le cœur du métier de l'app.
3. **Confort d'usage réel.** On découpe un concert le soir, au casque : une dominante chaude fatigue moins qu'un cyan saturé, et reste crédible « pro » grâce à la rigueur typographique (monospace tabulaire, filets, densité maîtrisée).
4. **Compatible avec la simplicité voulue.** A reprend la discipline de C (une seule couleur d'accent, hiérarchie sobre) tout en gardant une identité forte. On recommande d'ailleurs d'**emprunter à C ses cibles généreuses** (poignées de marqueurs 28 px, rayons un cran plus doux) — c'est intégré dans les specs ci-dessous.

**Ce qu'on garde des autres directions.** De B : l'infobulle de temps pendant le drag de marqueur et la minimap contrastée. De C : les cibles de clic larges et le ton rédactionnel direct.

**Point de vigilance.** Avec un accent unique chaud, les états sémantiques doivent rester discernables : le vert succès et le rouge erreur définis ci-dessous sont volontairement désaturés pour cohabiter avec le cuivre sans le concurrencer.

---

## 3. Système de design — Direction A « Alambic »

### 3.1 Tokens CSS complets

```css
/* ==========================================================
   Still — Design tokens. Thème sombre par défaut.
   Basculer le thème clair via [data-theme="light"] sur <html>.
   ========================================================== */
:root {
  /* --- Fonds --- */
  --bg:            #17130E;  /* fond app (brun-noir fumé) */
  --bg-deep:       #100D09;  /* zone waveform, minimap, champs */
  --panel:         #1F1A13;  /* panneau pistes, modales */
  --panel-2:       #262018;  /* boutons, hover de surface */
  --raised:        #2B241B;  /* hover de bouton, éléments saisis */

  /* --- Traits --- */
  --line:          #322A20;  /* bordures structurantes */
  --line-soft:     #262017;  /* grille temporelle, séparateurs faibles */

  /* --- Texte --- */
  --text:          #EFE6D8;  /* primaire */
  --text-2:        #B3A48D;  /* secondaire (labels, en-têtes) */
  --text-3:        #7D7060;  /* tertiaire (méta, hints, status bar) */
  --text-on-accent:#1C1207;  /* texte sur fond cuivre */

  /* --- Accent cuivre --- */
  --copper:        #E0883A;  /* accent principal */
  --copper-hi:     #F4B269;  /* hover, valeurs actives, timecode */
  --copper-lo:     #A85E24;  /* bordures actives, pressed */
  --copper-dim:    rgba(224, 136, 58, .14);  /* fonds sélectionnés */

  /* --- Waveform --- */
  --wave-l-peak:   #6E4A22;  /* canal G, couche peak (sombre) */
  --wave-l-rms:    #F4B269;  /* canal G, couche RMS (via dégradé copperL) */
  --wave-r-peak:   #5A3C1D;  /* canal D, couche peak */
  --wave-r-rms:    #D89050;  /* canal D, couche RMS — un cran plus doux que G */
  --wave-bg:       #100D09;  /* fond de la zone waveform */
  --wave-center:   #2B2318;  /* ligne médiane de chaque canal */
  --grid-time:     #262017;  /* verticales de la grille temporelle */
  --minimap-wave:  #A87B45;  /* waveform mono de la minimap */

  /* --- Curseurs & sélection --- */
  --playhead:      #F8EFE2;  /* tête de lecture (blanc chaud) */
  --playhead-glow: rgba(248, 239, 226, .65);
  --marker:        #E0883A;  /* ligne de marqueur */
  --marker-glow:   rgba(224, 136, 58, .50);
  --marker-flag-a: #E89043;  /* dégradé du fanion (haut) */
  --marker-flag-b: #C06D28;  /* dégradé du fanion (bas) */
  --selection:     rgba(244, 178, 105, .10); /* segment courant, plage minimap */

  /* --- Sémantique --- */
  --ok:            #8FBF6E;
  --ok-dim:        rgba(143, 191, 110, .10);
  --err:           #E0584A;
  --err-dim:       rgba(224, 88, 74, .10);
  --warn:          #D9A441;

  /* --- Focus --- */
  --focus-ring:    #F4B269;

  /* --- Géométrie --- */
  --r-s: 4px;   /* champs, kbd, petits éléments */
  --r-m: 7px;   /* boutons, lignes de liste, selects */
  --r-l: 11px;  /* modales, cartes */
  --r-round: 999px; /* chips, bouton play */

  /* --- Ombres --- */
  --shadow-pop:   0 8px 24px rgba(0,0,0,.45);                       /* menus, tooltips */
  --shadow-modal: 0 24px 64px rgba(0,0,0,.55), 0 2px 8px rgba(0,0,0,.40);

  /* --- Typo --- */
  --font-ui:   -apple-system, BlinkMacSystemFont, "Segoe UI Variable Text",
               "Segoe UI", system-ui, "Noto Sans", sans-serif;
  --font-mono: ui-monospace, "SF Mono", "Cascadia Mono", "JetBrains Mono",
               Menlo, Consolas, monospace;

  /* --- Mouvements --- */
  --ease: cubic-bezier(.25,.1,.25,1);
  --t-fast: 120ms;  /* hover, pressed */
  --t-med:  200ms;  /* ouverture panneau, modale */
}

/* ---------- Variante claire (option) ---------- */
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
  --copper-hi:     #A85E24;   /* en clair, le hover fonce */
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

### 3.2 Typographie

Aucune police téléchargée : stack système uniquement.

| Rôle | Police | Taille | Graisse | Notes |
|---|---|---|---|---|
| Corps UI (boutons, listes, labels) | `--font-ui` | 13 px | 400–500 | line-height 1.45 |
| Nom de piste (liste) | `--font-ui` | 13 px | 500 | ellipsis si trop long |
| Titres de modale | `--font-ui` | 16 px | 600 | |
| Wordmark « STILL » | `--font-ui` | 13 px | 600 | `letter-spacing:.30em`, all-small-caps |
| En-têtes de section (TRACKS, FORMAT…) | `--font-ui` | 11 px | 700 | uppercase, `letter-spacing:.12–.14em`, couleur `--text-2` |
| Timecode principal | `--font-mono` | 15 px | 500 | `font-variant-numeric: tabular-nums`, couleur `--copper-hi` |
| Durées, méta, règle temporelle | `--font-mono` | 10–12 px | 400 | tabular-nums partout où des chiffres bougent |
| Hints / status bar | `--font-ui` | 11 px | 400 | couleur `--text-3` |
| `kbd` | `--font-mono` | 10 px | 400 | fond `--panel-2`, bordure `--line`, bord bas 2 px |

**Règle d'or** : tout nombre susceptible de défiler (timecode, durées, pourcentages) est en monospace tabulaire — zéro tremblement de layout pendant la lecture.

### 3.3 Espacements, rayons, ombres

- Échelle d'espacement : **4 / 8 / 12 / 16 / 20 / 24 px** (base 4). Padding standard des conteneurs : 12–16 px ; gouttières de liste : 6–8 px.
- Rayons : `--r-s` 4 px (champs), `--r-m` 7 px (boutons, lignes), `--r-l` 11 px (modales), `--r-round` (chips, play).
- Ombres : uniquement `--shadow-pop` (tooltips, menus) et `--shadow-modal` (modales). **Aucune ombre sur les surfaces intégrées** — la profondeur vient des paliers de fond (`--bg-deep` < `--bg` < `--panel` < `--panel-2` < `--raised`).
- Bordures : 1 px `--line` pour la structure ; le hover éclaircit le fond avant d'éclaircir la bordure.

### 3.4 Spécification des zones

Fenêtre par défaut 1360×860 (min 1024×640). Ordre vertical : toolbar → règle → **waveform** → minimap → status bar ; panneau pistes à droite.

#### Toolbar — h 52 px, fond dégradé `#211B13 → #1C1710`, bordure basse `--line`
- Gauche : wordmark + goutte cuivre (18 px), séparateur, bouton **Open** (30 px de haut), puis nom du fichier (600) + méta mono (`1:12:36 · 44.1 kHz · stereo`, `--text-3`).
- Centre : transport — précédent / **play 34 px rond** (icône cuivre, bord `#4A3B26`) / suivant — puis timecode `27:35 / 1:12:36`.
- Droite : bouton primaire **Export…** (dégradé cuivre, texte `--text-on-accent`), toggle du panneau (icône, état `aria-pressed`).
- Boutons : h 30 px, padding 12 px, rayon `--r-m` ; icônes 12–14 px, trait 1.4–1.6.

#### Règle temporelle — h 26 px, fond `--bg-deep`
Ticks majeurs toutes les 10 min (libellés mono 10 px `--text-3`), mineurs toutes les 5 min ; densité adaptée au zoom (cible : un libellé tous les ~110 px minimum).

#### Zone waveform — flexible (≈ 620 px à 860 de haut), fond `--wave-bg` avec léger vignettage radial
- Deux canaux : centres à 27 % et 73 % de la hauteur, amplitude max 21 % ; ligne médiane `--wave-center` 1 px par canal.
- Rendu par canal : couche **peak** (barres 2 px, `--wave-*-peak`) sous une couche **RMS** (≈ 45 % de la hauteur peak, dégradé cuivre vertical `--copper-lo → --copper-hi → --copper-lo`). Canal droit un cran plus doux que le gauche pour lire la stéréo sans légende.
- **Segments** : entre deux marqueurs, chip centré en haut (top 12 px) — numéro seul (`1`), ou `3 · Ashes` pour le segment en lecture (chip plein cuivre, texte `--text-on-accent`). Fond du segment courant : `--selection`.
- **Marqueurs** : ligne 1 px `--marker` + glow ; fanion queue d'aronde 20 px de haut (`M1`… mono 10 px 700). **Cible de saisie : 28 px de large** (zone invisible centrée sur la ligne), curseur `ew-resize`. Hover : ligne 3 px. Drag : ligne 3 px + glow renforcé + infobulle mono `34:10.2` sous le fanion.
- **Playhead** : 1 px `--playhead` + glow + triangle 12 px pointe en bas au sommet. Toujours au-dessus des marqueurs (z-order : segments < grille < wave < marqueurs < playhead).

#### Minimap — h 64 px, sous la waveform, fond `--bg-deep`, bordure haute `--line`
- Waveform mono (`--minimap-wave`, opacité .75), marqueurs en traits 1 px cuivre 55 %, playhead 1 px blanc chaud.
- **Rectangle de viewport** : bordure 1 px `--copper`, fond `--copper-dim`, rayon 3 px, curseur `grab`/`grabbing`. Clic hors viewport = recentrage ; drag = pan ; molette sur la waveform = zoom (le rectangle se rétrécit).

#### Panneau pistes — w 284 px, escamotable (translation `--t-med`), fond `--panel`, bordure gauche `--line`
- En-tête h 40 px : `TRACKS` (11 px 700 uppercase) + `6 · 1:12:36` (mono, `--text-3`).
- Ligne : grille `26px 1fr auto`, h ≈ 38 px, rayon `--r-m` — numéro (mono, aligné droite), nom (500, ellipsis), durée (mono 11 px).
  - hover → fond `--panel-2` ; piste en lecture → fond `--copper-dim` + bordure `rgba(224,136,58,.28)` + numéro cuivre 700 ;
  - édition → input pleine largeur, fond `--bg-deep`, bordure `--copper-lo`.
- Pied h ≈ 36 px : hint `Double-click to rename` + `6 tracks`.
- Sélection dans la liste ⇄ surbrillance du segment sur la waveform (couplage bidirectionnel).

#### Status bar — h 28 px, fond `--bg-deep`, bordure haute `--line`
- Gauche : raccourcis en `kbd` + libellés : `Space Play/Pause · M Add marker · ← → Seek · ⌫ Delete marker`.
- Droite : format du fichier en mono : `WAV · 44.1 kHz · 16-bit · stereo`. La status bar sert aussi de zone de messages transitoires (« Marker added at 27:35 », 3 s).

#### Modal d'export — w 480 px, centrée, rayon `--r-l`, ombre `--shadow-modal`, backdrop `rgba(10,8,5,.62)` + blur 2 px
1. **Réglages** : titre `Export 6 tracks` + sous-titre fichier ; **Format** en segmented control (WAV / FLAC / MP3 / AAC — segment actif en dégradé cuivre) ; **Quality** (visible si MP3/AAC : 320/256/192/V0) et **Sample rate** sur une rangée ; **Destination** (chemin mono + `Choose…`) ; **File naming** (input mono `{n} - {title}`, hint : `Preview: 03 - Ashes.mp3 — placeholders: {n} {title} {date}`). Pied : `Cancel` / `Export 6 tracks` (primaire).
2. **Progression** : liste 6 lignes (`n° · nom · barre 4 px · statut mono` — `done` vert, `62%` cuivre sur fond `--copper-dim`, `waiting` gris) ; barre globale 7 px (dégradé cuivre) ; `Track 4 of 6` / `58% · ~1 min left` ; bouton `Cancel export`.
3. **Rapport** : bandeau succès (`--ok-dim` + coche ronde) `6 tracks exported` + chemin + poids total ; liste mono des fichiers (`✓ 01 - Intro & Tuning.mp3`…) ; pied `Show in Finder` (« Show in Explorer » sous Windows, « Show in Files » sous Linux) / `Done` (primaire).
4. **Erreur** (par piste) : la ligne passe en `--err-dim`, statut `failed` en `--err` ; le rapport liste `✗ 04 - Boreal.mp3 — destination is not writable` avec bouton `Retry failed`. L'export des autres pistes continue.

#### État vide (aucun fichier)
- Toolbar réduite (Open seul actif, transport et Export désactivés à 40 % d'opacité), panneau et minimap masqués.
- Au centre de la zone waveform : silhouette de waveform en pointillés cuivre 20 %, titre `Drop an audio file here` (16 px 600), sous-ligne `or press Open — WAV, FLAC, MP3, AAC · up to 4 hours` (`--text-3`), bouton `Open a file…`.
- Drag-over : bordure interne 2 px `--copper` en pointillés animés (désactivé si `prefers-reduced-motion`), fond `--copper-dim`.

#### État chargement / analyse
- Le nom du fichier apparaît immédiatement dans la toolbar.
- Zone waveform : la waveform **se dessine de gauche à droite** au fil de l'analyse (les colonnes analysées apparaissent en cuivre, le reste en `--line-soft`) — la barre de progression est la waveform elle-même ; libellé central discret `Analyzing… 42%` (mono).
- Annulable : `Esc` ou bouton `Cancel` sous le libellé.

### 3.5 Flux UX complet

1. **Ouvrir** — Drop ou `Open` → analyse avec waveform-progrès → à la fin, l'app est en édition : 1 piste unique « Track 01 », playhead à 0, minimap visible, panneau ouvert.
2. **Écouter / naviguer** — `Space` lecture-pause ; clic sur la waveform ou la minimap = seek ; `← →` ±5 s, `Shift+← →` ±30 s ; molette = zoom autour du curseur ; `↑ ↓` piste précédente/suivante (seek au début de piste).
3. **Marquer** — `M` pose un marqueur à la tête de lecture (ou double-clic sur la waveform à l'endroit voulu). Les numéros de pistes se recalculent instantanément ; la nouvelle piste apparaît dans le panneau avec un nom par défaut `Track 04`. Drag d'un fanion = ajustement fin (infobulle de temps, zoom auto léger aux bords). `⌫` sur un marqueur sélectionné le supprime (les deux pistes fusionnent, le nom de la première survit ; annulable).
4. **Nommer** — Double-clic sur le nom dans le panneau (ou `Enter` sur la ligne sélectionnée) → input inline, texte présélectionné ; `Enter` valide, `Esc` annule, `Tab` valide et passe à la piste suivante (flux « je nomme tout le set » sans toucher la souris).
5. **Exporter** — `Export…` → modale réglages (mémorise les derniers choix) → progression par piste + globale → rapport final avec `Show in Finder`. L'app reste utilisable pendant l'export (la modale peut être réduite en pilule de progression dans la toolbar).
6. **Undo/redo global** — `Cmd/Ctrl+Z` couvre pose/déplacement/suppression de marqueurs et renommages.

### 3.6 Micro-interactions

| Interaction | Comportement |
|---|---|
| Hover marqueur | Ligne 1→3 px, fanion s'éclaircit (`--t-fast`), curseur `ew-resize` |
| Drag marqueur | Glow renforcé, infobulle mono du temps sous le fanion, snap optionnel aux silences détectés (aimantation ±250 ms, désactivable avec `Alt`), les durées du panneau se mettent à jour en direct |
| Pose de marqueur (`M`) | Le fanion « tombe » de 6 px avec fondu 150 ms (aucune animation si `prefers-reduced-motion`) ; message status bar `Marker added at 27:35` |
| Renommage inline | Bordure cuivre + texte présélectionné ; validation = flash 300 ms `--copper-dim` sur la ligne |
| Play/pause | L'icône bascule sans animation ; le timecode cuivre est la seule chose qui « bouge » en permanence |
| Seek (clic) | Le playhead saute sans transition (précision avant tout), léger flash du triangle |
| Zoom | Centré sur la souris ; le rectangle de minimap s'anime en `--t-fast` |
| Toggle panneau | Translation 200 ms `--ease` ; la waveform se redimensionne en continu |
| Export lancé | Le bouton `Export…` devient pilule de progression `Exporting 58%` si la modale est réduite |
| Erreurs | Jamais de modale d'alerte pour les cas bénins : bandeau inline dans la modale d'export ou message status bar teinté `--err`. Ex. : `Can't read this file — format not supported (.aiff coming soon)` |
| Focus clavier | `outline: 2px solid var(--focus-ring); outline-offset: 2px` sur tout élément interactif |

### 3.7 Accessibilité & garde-fous

- Contrastes : `--text` sur `--bg` ≈ 13:1 ; `--text-2` ≈ 7:1 ; `--text-3` réservé aux méta non essentielles. `--text-on-accent` sur cuivre ≥ 7:1.
- `prefers-reduced-motion` : toutes les animations décoratives coupées (les barres de progression restent).
- Toutes les commandes de la toolbar accessibles au clavier ; la liste de pistes est une vraie liste navigable (`↑ ↓`, `Enter` pour renommer).
- Les glows (playhead, marqueurs) sont décoratifs : l'information reste lisible sans eux (lignes pleines).

---

## 4. Fichiers

- `design/direction-a.html` — Direction A « Alambic » (recommandée)
- `design/direction-b.html` — Direction B « Signal »
- `design/direction-c.html` — Direction C « Atelier »
- `design/DESIGN.md` — ce document

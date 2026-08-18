# Cahier des charges — AudioDistillery : atelier de traitement audio en masse

> Nom du produit : **AudioDistillery**. Nom court officiel : **Still** (l'alambic — à utiliser comme nom de binaire, commande CLI et identifiant technique). Extension du fichier projet : `.still`. Nom de repli si AudioDistillery devait être abandonné : **AudioStill**.

> Document destiné à servir de prompt initial et de référence pour Claude Code.
> À placer à la racine du projet (ex: `SPEC.md`) et à référencer dans le `CLAUDE.md`.

---

## 1. Vision du produit

Application desktop multi-plateforme (macOS, Windows, Linux) permettant de :

1. Charger des fichiers audio (WAV, FLAC, MP3, AIFF — extensible).
2. Visualiser la forme d'onde (waveform) avec zoom et navigation fluides.
3. Positionner, déplacer et supprimer des **marqueurs de début/fin de morceaux** directement sur la waveform.
4. Découper le fichier source en titres distincts selon ces marqueurs, **sans réencodage quand c'est possible** (découpe sample-accurate pour les formats non compressés).
5. Exporter les titres dans différents formats (WAV, FLAC, MP3, AAC) avec choix de la qualité/bitrate.

**Philosophie** : faire ce que fait WaveLab pour ce cas d'usage précis, mais en 10 fois plus simple. Chaque écran doit être compréhensible sans manuel.

**Principe fondateur — édition 100 % non destructive** : les fichiers audio sources ne sont **jamais modifiés, déplacés ni réécrits**. Toute opération (marqueurs, noms, chaîne de traitement, métadonnées) n'existe que sous forme de description dans le fichier projet, et ne se matérialise qu'à l'export, qui produit exclusivement de **nouveaux fichiers**. Voir §3 bis.

**Évolutions prévues (à anticiper dans l'architecture, pas à implémenter en phase 1)** :
- Phase 2 : édition de métadonnées ID3/Vorbis/MP4 (titre, artiste, album, pochette, numérotation automatique des pistes).
- Phase 3 : chaîne de traitement audio modulaire permettant l'intégration de plugins de mastering (architecture en pipeline dès le départ, hébergement VST3 plus tard).

---

## 2. Stack technique imposée

| Couche | Technologie | Rôle |
|---|---|---|
| Shell applicatif | **Tauri 2.x** | Fenêtrage, packaging multi-plateforme, pont front/back |
| Backend | **Rust** | 100 % de la logique métier |
| Décodage/analyse | **Symphonia** | Lecture des formats audio, extraction des données de waveform |
| Découpage/encodage | **FFmpeg** (binaire embarqué, appelé en sous-processus) | Découpe et export multi-formats |
| Frontend | **TypeScript + React** (ou Svelte si mieux justifié) | Affichage uniquement |
| Waveform UI | Rendu custom Canvas/WebGL alimenté par les données de peaks calculées côté Rust | Affichage de la forme d'onde et des marqueurs |

Note : ne pas utiliser WaveSurfer.js pour le décodage (il décoderait côté front, ce qui violerait la séparation des responsabilités — voir §3). Le front reçoit des **données de peaks pré-calculées** par le backend et ne fait que les dessiner.

---

## 3. RÈGLE ARCHITECTURALE NON NÉGOCIABLE : séparation stricte front / back

**Le frontend est un terminal d'affichage. Rien de plus.**

### Le frontend a le droit de :
- Afficher des données fournies par le backend (peaks de waveform, liste de marqueurs, métadonnées, progression d'export).
- Gérer la logique **purement visuelle** : zoom, scroll, survol, drag en cours (affichage temps réel du marqueur pendant le déplacement), thème, mise en page, animations, états de chargement.
- Convertir des coordonnées écran ↔ position temporelle **pour l'affichage uniquement**.
- Envoyer des **intentions** au backend via les commandes Tauri (ex: `add_marker(position_ms)`, `move_marker(id, new_position_ms)`, `export_tracks(config)`).

### Le frontend n'a JAMAIS le droit de :
- Décoder, lire ou toucher un fichier audio.
- Calculer les peaks de la waveform.
- Valider ou corriger des positions de marqueurs (snap au zero-crossing, bornes, chevauchements : c'est le backend qui décide et renvoie la position finale).
- Détenir l'état de référence du projet. L'état canonique (fichier chargé, marqueurs, config d'export) vit dans le backend ; le front n'en garde qu'une copie d'affichage synchronisée.
- Accéder au système de fichiers autrement que via les dialogues natifs Tauri, dont le résultat (chemin) est immédiatement transmis au backend.

### Le backend est responsable de :
- Tout l'état du projet (source de vérité unique), avec un modèle de données typé et sérialisable.
- Décodage, calcul des peaks (multi-résolution pour le zoom), analyse.
- Logique des marqueurs : validation, tri, snap optionnel au zero-crossing, contraintes.
- Découpage, encodage, export, gestion des erreurs FFmpeg.
- Persistance du projet (sauvegarde/chargement d'un fichier projet `.still` en JSON).

### Contrat d'interface
- Toutes les commandes Tauri sont documentées dans un fichier `src-tauri/COMMANDS.md` : nom, paramètres, retour, erreurs possibles.
- Les types partagés front/back sont générés depuis Rust (ex: `ts-rs` ou `specta`) — jamais dupliqués à la main.
- Les opérations longues (calcul de peaks, export) émettent des événements de progression ; le front les affiche, point.

**Test décisif** : on doit pouvoir remplacer entièrement le frontend (par une CLI, par exemple) sans toucher une ligne du backend. Si une fonctionnalité rend ce test impossible, elle est mal placée.

---

## 3 bis. RÈGLE PRODUIT NON NÉGOCIABLE : édition non destructive

**Le fichier source est sacré. Il n'est jamais ouvert en écriture.**

Concrètement :

- Tous les fichiers sources sont ouverts en **lecture seule**, sur toute la durée de vie de l'application. Aucun code, dans aucune couche, ne doit détenir un handle en écriture sur un fichier source.
- Marqueurs, découpes, renommages, métadonnées, chaîne de traitement : tout est stocké comme **description déclarative** dans le fichier projet (`.still`). Le projet est une recette, pas un résultat.
- L'export **crée toujours de nouveaux fichiers** dans le dossier de destination. Si la destination contient déjà un fichier du même nom, demander confirmation ou suffixer — jamais d'écrasement silencieux, et jamais dans le dossier du fichier source par défaut.
- Le tagging ID3 (phase 2) s'applique **aux fichiers exportés uniquement**, jamais au fichier source, même si celui-ci est un MP3 taggable.
- La future `ProcessingChain` (phase 3) s'applique à la volée à l'export ; aucun rendu intermédiaire ne remplace quoi que ce soit.
- Les fichiers de cache (peaks pré-calculés) vivent dans le répertoire de cache applicatif de l'OS, jamais à côté des sources.
- Annuler/refaire (undo/redo) porte sur l'état du projet, illimité dans la session, trivial à implémenter puisque tout est déclaratif.

**Test décisif** : à tout instant, on doit pouvoir supprimer l'application et le fichier projet, et retrouver ses fichiers sources octet pour octet identiques à leur état initial (vérifiable par checksum). Un test d'intégration automatisé doit le vérifier : checksum des sources avant/après un scénario complet (chargement → marqueurs → export).

---

## 4. Fonctionnalités — Phase 1 (MVP)

### 4.1 Chargement
- Ouverture par dialogue natif et par glisser-déposer.
- Formats : WAV, FLAC, MP3, AIFF.
- Le backend décode, calcule les peaks multi-résolution, renvoie durée, sample rate, canaux, format.
- Affichage d'une barre de progression pendant l'analyse des gros fichiers (> 1 h de programme, fichiers de plusieurs Go : à prendre en compte dès le départ — streaming, pas de chargement intégral en RAM).

### 4.2 Waveform et navigation
- Affichage stéréo (ou mono) avec zoom molette/trackpad centré sur le curseur, scroll horizontal, minimap globale.
- Lecture audio avec tête de lecture visible (la lecture peut être faite côté backend via rodio/cpal, ou via un flux fourni au front — au choix de l'implémentation, mais le décodage reste backend).
- Raccourcis clavier : espace = lecture/pause, M = poser un marqueur à la tête de lecture, flèches = navigation.

### 4.3 Marqueurs
- Un marqueur = une frontière entre deux titres (modèle "splits" plutôt que paires début/fin, avec possibilité de définir des zones à exclure — silences entre morceaux — en option).
- Pose au clic, déplacement au drag (pendant le drag : affichage front ; au relâchement : validation backend qui renvoie la position finale, avec snap zero-crossing optionnel).
- Liste latérale des titres résultants avec durées, renommage inline.
- Détection automatique de silences proposant des marqueurs (backend), que l'utilisateur valide/ajuste.

### 4.4 Découpage et export
- Panneau d'export : format cible, qualité/bitrate, dossier de destination, modèle de nommage (`{n} - {titre}`).
- Découpe sample-accurate. Export parallélisé si pertinent. Progression par piste + globale, annulable.
- Rapport final : liste des fichiers créés, erreurs éventuelles.

### 4.5 Projet
- Sauvegarde/chargement d'un fichier projet (référence au fichier source + marqueurs + noms + config d'export).

---

## 5. Design et UI — mobiliser une équipe design

L'UI doit être **belle, sobre et professionnelle** — niveau outil audio moderne, pas prototype d'ingénieur.

Instructions pour Claude Code :

1. **Avant d'écrire le moindre composant**, lancer une phase de design avec des sous-agents dédiés (équipe design) :
   - Un agent "direction artistique" : proposer 2–3 directions visuelles (thème sombre par défaut — standard des outils audio —, palette, typographie, iconographie), avec maquettes HTML statiques comparables.
   - Un agent "UX" : définir le flux utilisateur complet (ouvrir → marquer → nommer → exporter), les états vides, les états d'erreur, les raccourcis clavier.
   - Utiliser le skill `frontend-design` disponible dans l'environnement pour éviter tout rendu générique/template.
2. Me présenter les propositions pour arbitrage avant l'implémentation.
3. Contraintes : thème sombre par défaut (clair en option), waveform = élément central occupant l'essentiel de l'écran, panneau latéral escamotable pour la liste des titres, densité d'information maîtrisée, cibles de clic généreuses pour les marqueurs.

---

## 6. Qualité et organisation du code

- **Tests** : logique métier Rust couverte par des tests unitaires (validation de marqueurs, nommage, parsing) ; tests d'intégration sur le découpage avec fichiers de référence courts générés programmatiquement. Le front n'exige que des tests légers (conversion coordonnées, formatage).
- **Erreurs** : aucune erreur silencieuse. Toute erreur backend remonte au front avec un message actionnable en français.
- **CI** : GitHub Actions — build + tests sur les 3 OS dès le début du projet, packaging Tauri en artefacts.
- **Structure** : monorepo Tauri standard (`src/` front, `src-tauri/` back). Dans `src-tauri/`, séparer `core/` (logique métier pure, sans dépendance Tauri, testable isolément) et `commands/` (couche d'exposition Tauri, fine).
- **CLAUDE.md** à créer, rappelant les deux règles non négociables (§3 séparation front/back et §3 bis non-destructif) et leurs tests décisifs, pour que chaque session de travail les respecte.

---

## 7. Anticipation des phases suivantes (architecture seulement)

- **ID3 (phase 2)** : prévoir dans le modèle de piste un champ `metadata` extensible. La crate `lofty` sera utilisée. Ne rien implémenter en phase 1 au-delà du nom de piste.
- **Mastering (phase 3)** : le chemin audio d'export doit passer par une abstraction `ProcessingChain` (liste ordonnée de `Processor`), même si en phase 1 la chaîne est vide ou réduite à un gain unitaire. C'est cette abstraction qui accueillera plus tard normalisation, limiting, puis hébergement VST3.

---

## 8. Ordre de travail proposé pour Claude Code

1. Squelette Tauri + CI 3 OS + `CLAUDE.md` + `COMMANDS.md`.
2. Backend : chargement + décodage + peaks multi-résolution (avec tests).
3. Phase design (équipe design, §5) → arbitrage.
4. Front : waveform + zoom + lecture.
5. Marqueurs (backend d'abord, puis interaction front).
6. Export FFmpeg + progression + rapport.
7. Fichier projet + détection de silences.
8. Polish, packaging, icônes.

Chaque étape se termine par : tests verts sur les 3 OS en CI, démo fonctionnelle, et vérification explicite que le test décisif du §3 tient toujours.

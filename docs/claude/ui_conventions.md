# Conventions UI — frontend

> Règles UX / accessibilité pour le frontend Svelte (WebView Tauri).
> Miroir opérationnel de [`CLAUDE.md`](../../CLAUDE.md) — à respecter pour tout nouveau UI.

---

## Couleurs

**Aucune couleur en dur** (hex, `rgb()`, `rgba()`, `hsl()`, `white`, `black`) hors `src/variables.scss`.
`node scripts/check-colors.mjs` (branché sur `pnpm lint`) échoue sinon.

| Où                                 | Comment                                                                                                   |
| ---------------------------------- | --------------------------------------------------------------------------------------------------------- |
| Composant Svelte / SCSS            | token `$…` de `variables.scss` (`@use '…/variables.scss' as *;`)                                          |
| Canvas, worker, style inline       | `themeColor('token')` / `themeVar('token')` de `src/frontend/theme.ts`                                    |
| Nouveau token lu par un canvas     | l'ajouter à `$runtime-tokens` (`app.scss`) **et** à `THEME_TOKENS` (`theme.ts`) — `theme.test.ts` vérifie |
| Teinte translucide d'une couleur   | token existant (`$accent-soft`, `$error-border`…) ; sinon `rgba($token, a)` plutôt qu'un nouveau littéral |
| Worker (pas d'accès au stylesheet) | passer la couleur résolue dans le message (cf. `fill` de `waveform.worker.ts`)                            |

Familles : surfaces (`$surface-1…hover/active`), bordures (`$border-faint/subtle/strong`), texte
(`$text-strong/secondary/muted/subtle/faint`), accent (`$accent-wash/soft/strong/border/border-strong`),
sémantiques (`$color-success/warning/error` + `-soft` / `-border`), niveaux (`$meter-*`), clip audio
(`$clip-audio-*`). Choisir le token par **rôle**, pas par valeur ; ne pas créer de gris dédié à un écran.

Exceptions (hex autorisés, listés dans `check-colors.mjs`) : palettes d'identité dont la valeur est enregistrée
dans le projet ou sert de table — `frontend/trackColor.ts`, `frontend/modulation/colors.ts`,
`frontend/pluginFormatColor.ts`. Les couleurs dynamiques (`--track-color`, `--clip-color`, `--loc-color`)
viennent des données et restent des variables CSS.

---

## Curseur CSS

**Ne jamais utiliser `cursor: not-allowed`.**

Sur un élément désactivé (`:disabled`, `.disabled`, etc.) :

- garder l’opacité / le style visuel pour indiquer l’état ;
- utiliser explicitement `cursor: default` (curseur normal).

Le curseur « interdit » (cercle barré) est considéré comme une mauvaise UX dans ce projet.

---

## Focus clavier (`:focus-visible`)

Ressenti cible : **app bureau** — pas d’anneau de focus au clic souris ; indicateur uniquement pour la navigation clavier (Tab, etc.).

### Baseline

Dans `src/app.scss` :

- `:focus { outline: none }`
- `:focus-visible` global (ring `$accent-color`)
- `input` / `textarea` / `[contenteditable]` exclus du ring global (bordure / caret suffisent)
- conteneurs focus trap `[role="dialog"|"alertdialog"|"menu"]` exclus (sink `tabindex="-1"`, pas d’anneau sur le chrome à l’ouverture)

### Règles

| Cas                                               | Quoi faire                                                                                                                                 |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| Contrôle HTML natif (`button`, `a`, `[tabindex]`) | La règle globale suffit en général                                                                                                         |
| Indicateur custom                                 | Styler avec **`:focus-visible`**, jamais `:focus` seul                                                                                     |
| Widget custom (Knob, Slider, pads…)               | `:focus-visible` local ; si l’indicateur n’est pas un outline sur l’élément focusé, `outline: none` sur ce sélecteur + indicateur ailleurs |
| Parent qui réagit au focus clavier                | Préférer `:has(:focus-visible)` à `:focus-within`                                                                                          |
| Champ texte / spin / rename                       | `:focus` pour bordure d’édition = OK                                                                                                       |

`:focus` seul apparaît aussi au clic et après Échap (retour de focus) — à éviter pour les anneaux / highlights d’UI non-texte.

---

## Tooltips custom

Les bulles d’aide **ne sont pas focusables** :

| Élément                  | Règle                                                                                   |
| ------------------------ | --------------------------------------------------------------------------------------- |
| Bulle (`role="tooltip"`) | `tabindex="-1"`, hors cycle Tab ; `aria-hidden` si le texte est déjà sur le déclencheur |
| Icône « i »              | `tabindex="-1"` — survol uniquement, pas d’arrêt Tab dans les formulaires               |

Ne pas marquer un tooltip avec `data-es-focus-trap-portal` (réservé aux menus interactifs portailés).

Les `title=` natifs OS : OK, ne rien changer.

---

## Modales / dialogs (focus trap)

`aria-modal="true"` **ne suffit pas** : Tab peut sortir vers l’UI sous l’overlay.

Toute UI `role="dialog"` / `aria-modal="true"` (ou overlay équivalent) **doit** :

1. Brancher `use:focusTrap` (`src/frontend/focusTrap.ts`) sur le conteneur du dialog (`tabindex="-1"`).
2. Porter le backdrop vers `document.body` avec `use:portal` et rendre le shell non interactif avec `use:appInert` (ref-count sur `.app`) — voir `src/frontend/portal.ts`.
3. Dialogues empilés : un `use:focusTrap` par niveau (pile interne, seul le plus haut gère Tab).
4. Contenu porté hors du dialog (`document.body`, menus Select, etc.) : attribut `data-es-focus-trap-portal` pour rester dans le cycle Tab.
5. Option `initial: "container" | "first"` (défaut `container` pour dialogs ; `first` pour confirms / menus).

### Widget `Dialog`

Ne pas recâbler ces points à la main : utiliser `Dialog.svelte` (backdrop, `portal`,
`appInert`, `focusTrap`, `dismissable`, animation), avec `DialogHeader` (titre, icône,
bouton fermer) et `DialogFooter` (actions à droite, slot `start` à gauche). Props :
`label`, `width`, `height`, `initialFocus`. Seul `LibraryPopup` (popup ancrée, non
centrée) reste câblé à la main.

### Menus contextuels

Utiliser `Menu.svelte` + `MenuItem` / `MenuHeader` / `MenuSeparator` (focus trap, Échap,
repositionnement dans la fenêtre, tokens `$surface-overlay` / `$shadow-overlay`).
Sans `x`/`y`, le menu s'ancre sous son parent. Même piège de focus, sans `inert` global :

- conteneur `role="menu"` + `tabindex="-1"` + `use:focusTrap={{ initial: "first" }}` ;
- items `role="menuitem"` ;
- Échap ferme le menu (`use:dismissable`) ;
- ne pas re-focuser un parent juste après ouverture (laisser le trap focuser le 1er item).

### Déjà branché

- SettingsPanel / ProjectSettingsPanel
- MissingPluginsDialog / CrashSuspectDialog
- PluginPicker / LibraryPopup
- ModulationTargetPicker (trap sans inert — panneau inline)
- Menus contextuels (track / clip / locator / effect / automation / modulator type)
- CustomSelect (listbox)
- TrackStrip color pop
- GraphView tooltip custom

---

## Formulaires et réglages

Pour une page de réglages, composer avec `Section` (titre `h2`), `SubSection` (filet + titre
`h3`), `Field` (label, contrôle en slot, `hint` ou slot `hint` / `label`), `FieldGrid`,
`TextInput` et `TimeSignatureInput`, plutôt que de recopier `.field` / `.field-label` /
`.divider` dans le composant.

---

## Découper un gros composant

Un composant n'est découpé que s'il est vraiment difficile à lire (ordre de grandeur : plus de
1000 lignes). En dessous de ~800 lignes, ne pas découper.

Les sous-composants vont dans un dossier à côté du parent, nommé d'après lui :

| Dossier                 | Parent           | Contenu                                                    |
| ----------------------- | ---------------- | ---------------------------------------------------------- |
| `components/library/`   | `LibraryBrowser` | ligne d'asset, détail, état vide                           |
| `components/effects/`   | `EffectChain`    | en-tête d'effet, boutons des effets intégrés, corps plugin |
| `components/pianoRoll/` | `PianoRoll`      | barre d'outils, colonne des touches, lignes de contrôle    |
| `components/settings/`  | `SettingsPanel`  | une section de réglages par composant                      |
| `components/arrange/`   | `MainPanel`      | écran « aucune piste »                                     |

Règles :

- **Le parent garde** l'état partagé, les handlers, le glisser-déposer, les événements de
  pointeur et tout ce qui est rafraîchi à chaque mouvement de souris. L'enfant ne fait que
  de l'affichage : props en entrée, `createEventDispatcher` en sortie.
- **Refactor pur** : même HTML, mêmes valeurs de style, mêmes `stopPropagation`. Le HTML est
  copié, pas réécrit.
- **Le CSS ne traverse pas les composants.** Une règle du type `.parent.etat .enfant` devient
  une classe sur la racine de l'enfant (`unavailable`, `crashed`, `compact`), avec la règle
  réécrite dans l'enfant.
- **Sections de formulaire** : l'enfant reçoit sa part du brouillon par `bind:` (par exemple
  `bind:audio={draft.audio}`) ; le parent garde `draft`, Enregistrer et Annuler.
- **Dessin sur canvas** : les fonctions de dessin pures vont dans `src/frontend/<zone>/`
  (`gridRenderer`, `notesRenderer`, `velocityRenderer`), le parent garde la planification.
- Un style partagé par deux enfants est copié dans chacun, pas mis dans un fichier commun.
- Mettre à jour les docs qui nomment le composant (voir `change_impact_mapping.md`).

---

## Checklist nouveau widget / dialog

- [ ] Pas de `cursor: not-allowed`
- [ ] Focus visuel via `:focus-visible` (ou baseline globale)
- [ ] Tooltip custom : ni bulle ni « i » dans le cycle Tab (`tabindex="-1"`)
- [ ] Si dialog / modale : `Dialog` (qui apporte `focusTrap` + `portal` + `appInert`)
- [ ] Si menu contextuel / menu flottant : `Menu` (qui apporte `role="menu"` + `focusTrap` initial `first` + Échap)
- [ ] Si portail hors dialog : `data-es-focus-trap-portal` (menus interactifs uniquement)

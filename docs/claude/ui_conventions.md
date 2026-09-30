# Conventions UI — frontend

> Règles UX / accessibilité pour le frontend Svelte (WebView Tauri).
> Miroir opérationnel de [`CLAUDE.md`](../../CLAUDE.md) — à respecter pour tout nouveau UI.

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

| Cas | Quoi faire |
|---|---|
| Contrôle HTML natif (`button`, `a`, `[tabindex]`) | La règle globale suffit en général |
| Indicateur custom | Styler avec **`:focus-visible`**, jamais `:focus` seul |
| Widget custom (Knob, Slider, pads…) | `:focus-visible` local ; si l’indicateur n’est pas un outline sur l’élément focusé, `outline: none` sur ce sélecteur + indicateur ailleurs |
| Parent qui réagit au focus clavier | Préférer `:has(:focus-visible)` à `:focus-within` |
| Champ texte / spin / rename | `:focus` pour bordure d’édition = OK |

`:focus` seul apparaît aussi au clic et après Échap (retour de focus) — à éviter pour les anneaux / highlights d’UI non-texte.

---

## Tooltips custom

Les bulles d’aide **ne sont pas focusables** :

| Élément | Règle |
|---|---|
| Bulle (`role="tooltip"`) | `tabindex="-1"`, hors cycle Tab ; `aria-hidden` si le texte est déjà sur le déclencheur |
| Icône « i » | `tabindex="-1"` — survol uniquement, pas d’arrêt Tab dans les formulaires |

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

### Menus contextuels

Même piège de focus, sans `inert` global :
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

## Checklist nouveau widget / dialog

- [ ] Pas de `cursor: not-allowed`
- [ ] Focus visuel via `:focus-visible` (ou baseline globale)
- [ ] Tooltip custom : ni bulle ni « i » dans le cycle Tab (`tabindex="-1"`)
- [ ] Si dialog / modale : `use:focusTrap` + `use:portal` + `use:appInert`
- [ ] Si menu contextuel / menu flottant : `role="menu"` + `use:focusTrap={{ initial: "first" }}` + Échap
- [ ] Si portail hors dialog : `data-es-focus-trap-portal` (menus interactifs uniquement)

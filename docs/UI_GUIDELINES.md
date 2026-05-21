# UI Guidelines — Echoes Studio

> Ce document définit les principes UX, le système de design, et les règles d'interaction pour Echoes Studio. Toute contribution à l'interface doit s'y conformer.

---

## 1. Principes directeurs

### 1.1 Le problème des DAW actuels

Les DAW dominants (Ableton, Logic, FL Studio, Pro Tools) souffrent de problèmes structurels accumulés :

| Problème | Manifestation |
|----------|--------------|
| **Surcharge cognitive** | Centaines de boutons visibles en permanence, paramètres exposés même quand inutiles |
| **Menus profonds** | Options enfouies à 3-4 niveaux de sous-menus |
| **Incohérence des interactions** | Le clic-droit fait une chose ici, autre chose là ; le double-clic ouvre parfois une fenêtre, parfois édite un texte |
| **Onboarding impossible** | Aucun guidage contextuel ; l'utilisateur est largué devant une interface vide |
| **Workflows fragmentés** | Chaque action nécessite de naviguer entre plusieurs fenêtres détachées |
| **Absence de retour** | L'utilisateur ne sait pas si son action a fonctionné |

### 1.2 Les principes d'Echoes Studio

**Zéro friction d'entrée**
L'utilisateur doit pouvoir enregistrer une idée musicale en moins de 10 secondes après ouverture de l'app. Toute étape de setup obligatoire est un échec de design.

**Cohérence absolue**
Le même geste fait toujours la même chose. Si le double-clic renomme un élément à un endroit, il renomme partout. Si le drag déplace à un endroit, il déplace partout. Pas d'exceptions.

**Affordance visible**
Chaque élément interactif doit communiquer son comportement sans tooltip obligatoire. La forme, la couleur, et le curseur suffisent.

**Densité maîtrisée**
N'afficher que ce qui est pertinent au contexte actuel. Les contrôles avancés sont accessibles mais ne surchargent pas l'interface de base.

**Feedback immédiat**
Toute action produit un retour visuel instantané. Pas de latence perçue entre le geste et la réponse.

**Progressivité**
L'interface de base est simple. La complexité se révèle au fur et à mesure que l'utilisateur explore — jamais imposée d'entrée.

---

## 2. Système de design

### 2.1 Couleurs

```scss
// Fondamentaux
$color-bg-base:     #1a1a1f;   // Fond principal
$color-bg-panel:    #222229;   // Fond des panneaux
$color-bg-elevated: #2c2c34;   // Éléments surélevés (modales, tooltips)
$color-border:      #3c3c43;   // Séparateurs, bordures
$color-border-subtle: #2e2e35; // Bordures discrètes

// Texte
$color-text-primary:   #f7f7f9; // Texte principal
$color-text-secondary: #9999aa; // Labels, descriptions
$color-text-disabled:  #55555f; // Éléments désactivés

// Accent
$color-accent:         #1571ff; // Action principale, sélection
$color-accent-hover:   #2a7fff; // Hover sur accent
$color-accent-active:  #0f5bd4; // Active/pressed

// États
$color-success:  #22c55e;
$color-warning:  #f59e0b;
$color-error:    #ef4444;
$color-record:   #ef4444; // Rouge = enregistrement actif

// Clips (palette par piste — [TODO: à définir])
$color-clip-default: #1571ff;
```

**Règles d'utilisation :**
- Jamais de blanc pur (`#ffffff`) sur fond sombre — utiliser `$color-text-primary`
- La couleur accent (`$color-accent`) est réservée aux actions et sélections — pas décorative
- Le rouge (`$color-record`) est réservé à l'enregistrement — ne pas l'utiliser ailleurs

### 2.2 Typographie

```scss
// Police principale
$font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
// [TODO: Décider si Inter est embarquée ou utilise la police système]

// Tailles
$text-xs:   10px;   // Labels très compacts (valeurs de paramètres)
$text-sm:   12px;   // Labels courants, menus
$text-base: 14px;   // Texte par défaut
$text-lg:   16px;   // Titres de section
$text-xl:   20px;   // Titres principaux

// Poids
$font-regular: 400;
$font-medium:  500;
$font-semibold: 600;
```

**Règles :**
- Pas de texte sous 10px (lisibilité)
- Les valeurs numériques (BPM, volume, fréquence) utilisent une police à chasse fixe pour éviter le "saut" lors de changements de valeur : `font-variant-numeric: tabular-nums`
- Éviter les majuscules pour les labels longs (lisibilité)

### 2.3 Espacements

Système de grille de 4px :

```scss
$space-1: 4px;
$space-2: 8px;
$space-3: 12px;
$space-4: 16px;
$space-6: 24px;
$space-8: 32px;
```

**Règles :**
- Tout espacement est un multiple de 4px
- Padding interne des composants : minimum `$space-2` (8px)
- Séparation entre sections logiques : `$space-4` (16px) minimum

### 2.4 Rayons de coins

```scss
$radius-sm: 3px;   // Boutons, tags
$radius-md: 6px;   // Panneaux, cards
$radius-lg: 10px;  // Modales
$radius-full: 50%; // Knobs, boutons circulaires
```

### 2.5 Icônes

[TODO: Choisir une librairie d'icônes — Lucide, Phosphor, ou icônes custom SVG]

**Règles :**
- Taille standard : 16×16px (inline), 20×20px (boutons), 24×24px (toolbar)
- Style : outline (pas filled) pour les actions ; filled pour les états actifs
- Chaque icône doit avoir un `aria-label` ou être accompagnée d'un label texte

### 2.6 Ombres et profondeur

```scss
$shadow-sm:  0 1px 3px rgba(0,0,0,0.4);   // Éléments légèrement surélevés
$shadow-md:  0 4px 12px rgba(0,0,0,0.5);  // Modales, menus flottants
$shadow-lg:  0 8px 24px rgba(0,0,0,0.6);  // Drawers, panels détachés
```

---

## 3. Composants UI

### 3.1 Boutons

```
Hiérarchie :
[Primary]    — Fond accent, action principale (ex: Export)
[Secondary]  — Fond panel, action secondaire
[Ghost]      — Transparent, bordure subtile
[Danger]     — Fond rouge, actions destructives
[Icon-only]  — Icône sans texte, tooltip obligatoire
```

**Règles :**
- Maximum 1 bouton Primary visible par zone de focus
- Les boutons désactivés ne s'affichent qu'avec `opacity: 0.4`, jamais cachés
- Hauteur standard : 28px (compact), 36px (normal)
- Toujours un état `:hover`, `:active`, `:focus-visible`

### 3.2 Knobs (contrôles rotatifs)

Les knobs sont utilisés pour les paramètres continus (volume, pan, effets). Comportement :
- **Drag vertical** : monter = augmenter, descendre = diminuer
- **Shift + drag** : precision mode (mouvement 10× ralenti)
- **Double-clic** : saisie valeur directe
- **Clic droit** : menu contextuel (Reset to default, Enter value, Assign MIDI)
- **Scroll molette** : incrément de ±1 unité

### 3.3 Timeline / Clips

- **Clic sur clip** : sélectionner
- **Double-clic sur clip** : ouvrir l'éditeur du clip
- **Drag clip** : déplacer sur la timeline (avec snap)
- **Alt + drag clip** : dupliquer
- **Resize bord droit/gauche** : trim du clip
- **Clic sur zone vide** : désélectionner
- **Clic sur règle temporelle** : positionner la tête de lecture

### 3.4 Pistes (Tracks)

- **Double-clic sur nom** : renommer
- **Bouton Mute (M)** : sourdine (clip visuellement atténué)
- **Bouton Solo (S)** : solo (toutes les autres pistes mutées)
- **Drag de la hauteur de piste** : redimensionner
- **Clic droit sur la piste** : menu contextuel (Duplicate, Delete, Color...)

---

## 4. Patterns d'interaction

### 4.1 Raccourcis clavier

**Transport**
| Action | Raccourci |
|--------|-----------|
| Play / Pause | `Espace` |
| Stop (retour à 0) | `Entrée` |
| Rewind | `Entrée` (x2 rapide) |
| Arm enregistrement | `R` |
| Loop toggle | `L` |

**Édition**
| Action | Raccourci |
|--------|-----------|
| Undo | `Ctrl/Cmd + Z` |
| Redo | `Ctrl/Cmd + Shift + Z` |
| Couper | `Ctrl/Cmd + X` |
| Copier | `Ctrl/Cmd + C` |
| Coller | `Ctrl/Cmd + V` |
| Supprimer | `Suppr` ou `Backspace` |
| Sélectionner tout | `Ctrl/Cmd + A` |
| Dupliquer | `Ctrl/Cmd + D` |

**Navigation**
| Action | Raccourci |
|--------|-----------|
| Zoom in timeline | `Ctrl/Cmd + Scroll` ou `+` |
| Zoom out timeline | `Ctrl/Cmd + Scroll` ou `-` |
| Zoom to fit | `Ctrl/Cmd + 0` |

[TODO: Liste complète à définir et rendre configurable en Phase 5]

### 4.2 Drag & Drop

**Règles :**
- Le drag est initialisé après 3px de mouvement (évite les clics accidentels)
- Un indicateur visuel (ligne de drop) montre la destination avant le drop
- Les éléments non-droppables rejettent visuellement la cible (curseur `not-allowed`)
- L'undo s'applique aux drag & drop

**Cas supportés :**
- Clip → autre piste : déplacer ou copier (+ Alt)
- Fichier audio depuis l'OS → piste : importer
- [TODO: autres cas à définir]

### 4.3 Menus contextuels

Accès via clic droit uniquement. **Règles :**
- Maximum 8 entrées visibles sans sous-menu
- Pas plus d'1 niveau de sous-menu
- Les actions destructives sont en bas, séparées par un divider
- Les raccourcis sont affichés à droite de chaque entrée

### 4.4 Tooltips

- Délai d'apparition : 500ms (évite le bruit visuel)
- Position : toujours au-dessus ou en-dessous, jamais masquant la cible
- Contenu : label de l'action + raccourci clavier si applicable
- Les tooltips sont obligatoires pour les boutons icon-only

---

## 5. Règles pour les contributeurs UI

### À faire

- Utiliser les variables SCSS existantes — jamais de valeurs hardcodées
- Tester chaque composant avec le clavier (navigation Tab, focus visible)
- Vérifier le comportement à différents niveaux de zoom OS (100%, 125%, 150%)
- Respecter les ratios de contraste WCAG AA minimum (4.5:1 pour le texte)
- Tester sur Windows ET macOS (les polices et WebView peuvent différer)

### À éviter

- Cacher des informations derrière des hovers obligatoires (problème sur touchpad/tablet)
- Utiliser des animations > 300ms pour des interactions fréquentes
- Introduire un nouveau pattern d'interaction sans discussion préalable
- Modifier les variables SCSS globales sans valider l'impact sur tous les composants
- Ajouter des dépendances UI externes sans justification (garder le bundle léger)

### Process de contribution UI

1. Ouvrir une issue avec une maquette (même rough) avant de coder
2. Référencer ce document dans la PR et cocher les règles respectées
3. Joindre des captures d'écran Windows + macOS dans la PR

---

## Liens

- [ARCHITECTURE.md](ARCHITECTURE.md) — Structure des composants Svelte
- [CONTRIBUTING.md](../CONTRIBUTING.md) — Process de contribution général
- [TODO: Lien vers le fichier Figma/Penpot du design system]

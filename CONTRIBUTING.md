# Contributing to Echoes Studio

Merci de votre intérêt pour Echoes Studio ! Toutes les formes de contribution sont les bienvenues : code, design, tests, documentation, retours utilisateurs.

---

## Table des matières

- [Avant de commencer](#avant-de-commencer)
- [Setup de l'environnement](#setup-de-lenvironnement)
- [Types de contributions](#types-de-contributions)
- [Conventions de code](#conventions-de-code)
- [Conventions de commit](#conventions-de-commit)
- [Processus Pull Request](#processus-pull-request)
- [Signaler un bug](#signaler-un-bug)
- [Code de conduite](#code-de-conduite)

---

## Avant de commencer

1. **Consultez la [ROADMAP.md](ROADMAP.md)** pour voir où en est le projet et ce qui est prioritaire.
2. **Vérifiez les [issues ouvertes](https://github.com/[TODO:org]/echoes-studio/issues)** pour éviter les doublons.
3. Pour les changements importants (nouvelle fonctionnalité, refactoring majeur), **ouvrez une issue d'abord** pour en discuter avant de coder.

---

## Setup de l'environnement

### Prérequis

- Node.js ≥ 18
- pnpm ≥ 8 (`npm install -g pnpm`)
- Rust stable (via [rustup](https://rustup.rs))
- Dépendances système Tauri (voir [README.md](README.md#installation))

### Installation

```bash
git clone https://github.com/[TODO:org]/echoes-studio.git
cd echoes-studio
pnpm install
pnpm check-env      # vérifie l'environnement (optionnel — lancé auto par tauri:dev)
pnpm tauri:dev
```

Pour les détails par OS, voir [docs/BUILD.md](docs/BUILD.md).

### Vérifications avant de contribuer

Une seule commande lance la suite complète :

```bash
pnpm test           # check + lint + cargo fmt + cargo clippy + cargo test
```

Ou les étapes individuellement :

```bash
pnpm check          # TypeScript + Svelte
pnpm lint           # ESLint + Prettier
pnpm format         # auto-formatage

# Côté Rust (équivalent à ce que `pnpm test` exécute)
cd src-tauri
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

---

## Types de contributions

### Code

- Corrigez des bugs de la liste d'issues taguées `bug`
- Implémentez des fonctionnalités de la phase courante de la ROADMAP
- Améliorez les performances ou réduisez la dette technique

### Design & UX

- Proposez des maquettes (Figma, Penpot, ou wireframes) via une issue
- Signalez des incohérences d'interaction ou des problèmes d'ergonomie
- Consultez [docs/UI_GUIDELINES.md](docs/UI_GUIDELINES.md) avant toute contribution UI

### Tests

- Ajoutez des tests unitaires Rust (`src-tauri/src/`)
- Ajoutez des tests de composants Svelte
- Rédigez des cas de test pour l'audio engine

### Documentation

- Améliorez les fichiers `docs/`
- Corrigez les fautes ou les inexactitudes
- Ajoutez des exemples de code

---

## Conventions de code

### TypeScript / Svelte

- **Formatage** : Prettier (`.prettierrc` à la racine) — `pnpm format` avant chaque commit
- **Linting** : ESLint (`.eslintrc.cjs`) — zéro warning toléré
- **Nommage** :
  - Variables et fonctions : `camelCase`
  - Composants Svelte : `PascalCase`
  - Constantes globales : `UPPER_SNAKE_CASE`
- **Langue** : tout le code (variables, commentaires, identifiants) en **anglais**
- **Commentaires** : uniquement quand le "pourquoi" n'est pas évident — éviter les commentaires qui répètent le code
- **Types** : toujours typer explicitement les interfaces et les retours de fonctions

### Rust

- **Formatage** : `cargo fmt` obligatoire
- **Linting** : `cargo clippy` — zéro warning toléré
- **Nommage** : conventions Rust standards (`snake_case` pour les fonctions, `PascalCase` pour les types)
- **Erreurs** : utiliser `Result<T, String>` pour les commandes Tauri, types d'erreur dédiés en interne
- **Langue** : anglais

### Svelte (spécifique)

- Préférer les Svelte stores aux props drilling
- Un composant = une responsabilité unique
- Les effets coûteux (audio, canvas) vont dans des Web Workers

---

## Conventions de commit

Utiliser le format **Conventional Commits** :

```
<type>(<scope>): <description courte>

[corps optionnel — explication du pourquoi si non évident]

[footer optionnel — refs issues, breaking changes]
```

### Types

| Type | Usage |
|------|-------|
| `feat` | Nouvelle fonctionnalité |
| `fix` | Correction de bug |
| `refactor` | Refactoring sans changement de comportement |
| `style` | Formatage, CSS (pas de changement logique) |
| `test` | Ajout ou modification de tests |
| `docs` | Documentation uniquement |
| `chore` | Tâches de build, dépendances, config |
| `perf` | Amélioration de performance |

### Scopes courants

`audio`, `ui`, `timeline`, `transport`, `clip`, `track`, `store`, `tauri`, `rust`, `build`

### Exemples

```
feat(transport): implement play/pause via cpal output stream
fix(waveform): fix canvas size to match clip dimensions
refactor(store): migrate track state to svelte writable stores
docs(architecture): add audio threading model diagram
chore(deps): upgrade tauri to 1.6.0
```

### Règles

- Description en anglais, temps présent (impératif)
- Pas de majuscule au début, pas de point final
- Longueur de la description ≤ 72 caractères
- Référencer les issues avec `Closes #123` ou `Refs #456`

---

## Processus Pull Request

### Avant d'ouvrir une PR

1. **Branch depuis `main`** : `git checkout -b feat/mon-feature`
2. **Commits atomiques** : chaque commit doit être cohérent et buildable
3. **Tests** : ajouter des tests pour les nouvelles fonctionnalités
4. **Vérifications** :
   ```bash
   pnpm test
   ```

### Template de PR

```markdown
## Description
[Que fait ce changement ? Pourquoi ?]

## Type de changement
- [ ] Bug fix
- [ ] Nouvelle fonctionnalité
- [ ] Refactoring
- [ ] Documentation

## Lié à
Closes #[numéro d'issue]

## Checklist
- [ ] Le code passe `pnpm check` et `pnpm lint`
- [ ] `cargo clippy` et `cargo test` passent
- [ ] J'ai ajouté des tests pour mon changement
- [ ] La documentation est mise à jour si nécessaire
- [ ] J'ai testé manuellement sur [Windows / macOS / Linux]
```

### Review

- Minimum **1 approbation** requise pour merger sur `main`
- Les suggestions de la review doivent être traitées avant merge
- Utiliser "Resolve conversation" une fois un commentaire adressé
- Préférer le **squash merge** pour les features, le **merge commit** pour les releases

---

## Signaler un bug

Ouvrir une issue avec le template suivant :

```markdown
**Version** : 0.x.x
**OS** : Windows 11 / macOS 14 / Ubuntu 22.04
**Description** : [Que s'est-il passé ?]
**Reproduction** : [Étapes pour reproduire]
**Comportement attendu** : [Ce qui devrait se passer]
**Logs** : [Coller les logs de la console Tauri si disponibles]
```

---

## Code de conduite

Echoes Studio s'engage à maintenir un environnement ouvert et respectueux pour tous les contributeurs.

En résumé :
- Soyez respectueux et constructif dans vos échanges
- Accueillez les contributeurs de tous niveaux
- Acceptez les critiques constructives
- Concentrez-vous sur ce qui est meilleur pour le projet et la communauté

Les comportements inacceptables (harcèlement, discrimination, attaques personnelles) entraîneront une exclusion de la communauté du projet.

→ [TODO: Lien vers le CODE_OF_CONDUCT.md complet]

---
name: ui-controls
description: Build or change any Svelte UI that contains a button, field, select, tab/segmented group, toggle, slider, knob, menu or list row. Use so new UI reuses the shared widgets and controls.scss mixins instead of restyling.
---

# UI Controls

## Objective

Every widget looks and behaves like the others. New UI is assembled from the shared
components and mixins; it never carries its own border, radius or hover colors.

## Workflow

1. Read `.claude/rules/ui_controls_rules.md` and `docs/claude/ui_conventions.md`
   § « Contrôles » (catalogue, sizes, states).
2. Pick the widget for the job (table in the doc):
   - choice among options → `SegmentedControl` (or `segmented` + `segment` mixins when the
     markup is custom), long list → `CustomSelect`;
   - action → `Button` in forms and dialogs, `button` / `chip` / `icon-button` mixins in dense
     panels;
   - text / number → `TextInput`, `NumberInput`, or the `field` mixin;
   - on/off → `Toggle` (switch row) or a button with `control-on` (toolbar toggle);
   - value → `RangeSlider`, `StepSlider`, `Knob` / `ParamKnob`.
3. In the component: `@use '../controls.scss' as *;` then `@include`. Add only layout
   (`flex`, `width`, `margin`, `position`). Heights and radii are tokens.
4. States: selected = `control-on`; danger = `control-on-danger` / `icon-button-danger`;
   warning / success = `control-on-warning` / `control-on-success`. Disabled keeps its look
   (opacity .4) with `cursor: default`.
5. Keep the project rules: no hard-coded color, `:focus-visible` only, `Dialog` / `Menu` for
   overlays, tooltips out of the Tab cycle.
6. Verify: `pnpm lint` (includes `check-controls.mjs`), `pnpm check`, `pnpm test:frontend`.
   Look at the result in the app when possible and say when it could not be checked.

## If the design needs something the mixins lack

Extend `src/controls.scss` (a mixin, not a local override), apply it where it fits, and
document it in `docs/claude/ui_conventions.md` in the same change. A clickable surface that is
not a standard control goes in `CUSTOM_SURFACES` of `scripts/check-controls.mjs` with a reason.

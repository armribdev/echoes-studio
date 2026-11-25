# UI Controls Rules (one look for every widget)

Every button, field, dropdown, segmented group, slider and knob shares one look. New UI
**reuses it — never restyles it.** Catalogue, sizes, states and recipes:
`docs/claude/ui_conventions.md` § « Contrôles ». Code: `src/controls.scss` (mixins),
`src/variables.scss` (tokens), shared components in `src/components/`.

**Before writing any interactive element:**

1. Is there a shared component? `Button`, `SegmentedControl`, `CustomSelect`, `TextInput`,
   `NumberInput`, `Toggle`, `RangeSlider`, `StepSlider`, `Knob` / `ParamKnob`, `Menu` /
   `MenuItem`, `Dialog`. Use it.
2. Otherwise `@use '../controls.scss' as *;` and `@include` the mixin that matches
   (`button`, `chip`, `icon-button`, `button-square`, `segmented` + `segment`, `field`,
   `list-item`, `power-button`, `range-slider`, `control-on*`).
3. A new look is a change to `controls.scss` (then document it), never a local copy.

**Forbidden in a component style block:**

- Hand-written `border`, `border-radius`, hover / selected / disabled colors on a clickable
  element (the mixins own them). Radii and heights come from tokens (`$radius-control`,
  `$ctrl-height-xs` 20 / `-sm` 24 / `$ctrl-height` 30, `$menu-item-height`), not `4px`, `22px`…
- A second visual language for the same job: « selected » is always `control-on`
  (`$accent-soft` + `$accent-border` + `$accent-text`); solid `$accent-color` is only for
  `Button primary` and switches.
- `cursor: not-allowed`, `accent-color`, hand-styled `::-webkit-slider-thumb`.
- A native `<select>` (use `CustomSelect`).

**Enforced:** `pnpm lint` runs `scripts/check-controls.mjs`. A rule with `cursor: pointer`
that includes no `controls.scss` mixin fails the build. A surface that is truly not a
standard control (clip, pad, ruler, color swatch…) is added to `CUSTOM_SURFACES` in that
script **with its reason** — that is a design decision, say so in the summary.

> Pairs with the colors rule (`check-colors.mjs`) and the focus / dialog / menu rules in
> `CLAUDE.md`. If the mixins cannot express what the design needs, extend them and update
> `docs/claude/ui_conventions.md` in the same change.

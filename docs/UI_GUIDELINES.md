# UI Guidelines — Echoes Studio

> This document defines UX principles, the design system, and interaction rules for Echoes Studio. All UI contributions must follow it.

---

## 1. Core Principles

### 1.1 Problems with Existing DAWs

The dominant DAWs (Ableton, Logic, FL Studio, Pro Tools) suffer from accumulated structural problems:

| Problem | Manifestation |
|---------|--------------|
| **Cognitive overload** | Hundreds of buttons always visible; parameters exposed even when irrelevant |
| **Deep menus** | Options buried 3–4 levels deep in sub-menus |
| **Inconsistent interactions** | Right-click does one thing here, something else there; double-click sometimes opens a window, sometimes edits text |
| **Impossible onboarding** | No contextual guidance; users are left in front of an empty interface |
| **Fragmented workflows** | Every action requires navigating between multiple detached windows |
| **No feedback** | The user cannot tell whether an action succeeded |

### 1.2 Echoes Studio Principles

**Zero entry friction**
The user must be able to record a musical idea within 10 seconds of opening the app. Any mandatory setup step is a design failure.

**Absolute consistency**
The same gesture always does the same thing. If double-click renames something in one place, it renames everywhere. If drag moves in one place, it moves everywhere. No exceptions.

**Visible affordance**
Every interactive element communicates its behaviour without requiring a tooltip. Shape, colour, and cursor are sufficient.

**Controlled density**
Show only what is relevant to the current context. Advanced controls are accessible but do not clutter the default interface.

**Immediate feedback**
Every action produces instant visual feedback. No perceived latency between gesture and response.

**Progressive disclosure**
The base interface is simple. Complexity is revealed as the user explores — never forced upfront.

---

## 2. Design System

### 2.1 Colours

```scss
// Backgrounds
$color-bg-base:       #1a1a1f;   // Main background
$color-bg-panel:      #222229;   // Panel backgrounds
$color-bg-elevated:   #2c2c34;   // Elevated elements (modals, tooltips)
$color-border:        #3c3c43;   // Separators, borders
$color-border-subtle: #2e2e35;   // Subtle borders

// Text
$color-text-primary:   #f7f7f9;  // Primary text
$color-text-secondary: #9999aa;  // Labels, descriptions
$color-text-disabled:  #55555f;  // Disabled elements

// Accent
$color-accent:        #1571ff;   // Primary action, selection
$color-accent-hover:  #2a7fff;   // Hover on accent
$color-accent-active: #0f5bd4;   // Active/pressed state

// Status
$color-success: #22c55e;
$color-warning: #f59e0b;
$color-error:   #ef4444;
$color-record:  #ef4444;         // Red = recording active
```

**Usage rules:**
- Never use pure white (`#ffffff`) on a dark background — use `$color-text-primary`
- `$color-accent` is reserved for actions and selections — not decoration
- `$color-record` (red) is reserved for recording state — do not use elsewhere

### 2.2 Track Colours

Tracks are assigned a colour from a cycling palette of 8:

```scss
$track-colors: (
  #4f8ef7,  // blue
  #f7824f,  // orange
  #4ff79e,  // green
  #f74f8e,  // pink
  #a04ff7,  // purple
  #f7d44f,  // yellow
  #4ff7f0,  // cyan
  #f74f4f,  // red
);
```

### 2.3 Typography

```scss
$font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
// [TODO: decide whether Inter is bundled or uses the system font]

$text-xs:   10px;   // Compact labels (parameter values)
$text-sm:   12px;   // Common labels, menus
$text-base: 14px;   // Default text
$text-lg:   16px;   // Section headings
$text-xl:   20px;   // Main headings

$font-regular:  400;
$font-medium:   500;
$font-semibold: 600;
```

**Rules:**
- No text below 10px (legibility)
- Numeric values (BPM, volume, frequency) use tabular figures to prevent layout shift: `font-variant-numeric: tabular-nums`
- Avoid all-caps for long labels (readability)

### 2.4 Spacing

4px grid system:

```scss
$space-1: 4px;
$space-2: 8px;
$space-3: 12px;
$space-4: 16px;
$space-6: 24px;
$space-8: 32px;
```

**Rules:**
- All spacing is a multiple of 4px
- Minimum internal component padding: `$space-2` (8px)
- Separation between logical sections: `$space-4` (16px) minimum

### 2.5 Border Radius

```scss
$radius-sm:   3px;   // Buttons, tags
$radius-md:   6px;   // Panels, cards
$radius-lg:   10px;  // Modals
$radius-full: 50%;   // Knobs, circular buttons
```

### 2.6 Icons

[TODO: Choose an icon library — Lucide, Phosphor, or custom SVGs]

**Rules:**
- Standard sizes: 16×16px (inline), 20×20px (buttons), 24×24px (toolbar)
- Style: outline for actions; filled for active states
- Every icon must have an `aria-label` or an accompanying text label

### 2.7 Shadows and Depth

```scss
$shadow-sm: 0 1px 3px rgba(0,0,0,0.4);   // Slightly elevated elements
$shadow-md: 0 4px 12px rgba(0,0,0,0.5);  // Modals, floating menus
$shadow-lg: 0 8px 24px rgba(0,0,0,0.6);  // Drawers, detached panels
```

---

## 3. UI Components

### 3.1 Buttons

```
Hierarchy:
[Primary]    — Accent background, main action (e.g. Export)
[Secondary]  — Panel background, secondary action
[Ghost]      — Transparent, subtle border
[Danger]     — Red background, destructive actions
[Icon-only]  — Icon without text; tooltip required
```

**Rules:**
- Maximum 1 Primary button visible per focus zone
- Disabled buttons use `opacity: 0.4` — never hidden
- Standard heights: 28px (compact), 36px (normal)
- Always define `:hover`, `:active`, `:focus-visible` states

### 3.2 Knobs (rotary controls)

Knobs are used for continuous parameters (volume, pan, effects). Behaviour:
- **Vertical drag** — up = increase, down = decrease
- **Shift + drag** — precision mode (movement slowed 10×)
- **Double-click** — reset to `defaultValue`
- **Right-click** — context menu (Reset to default, Enter value, Assign MIDI)
- **Scroll wheel** — increment by ±1 unit

Current implementation (`Knob.svelte`):
- SVG arc render: grey track + accent arc
- Geometry: 298.8° sweep (7:30 → 4:30 clockwise)
- Labels `VOL` / `PAN` displayed above knobs

### 3.3 Timeline / Clips

- **Click clip** — select
- **Double-click clip** — open clip editor
- **Drag clip** — move on timeline (with snap)
- **Alt + drag clip** — duplicate
- **Resize left/right edge** — trim clip
- **Click empty area** — deselect
- **Click on ruler** — position playhead

### 3.4 Tracks

- **Double-click name** — rename (inline `contentEditable`)
- **Click M button** — mute (clip visually dimmed)
- **Click S button** — solo (all other tracks muted; exclusive)
- **Drag track height** — resize
- **Right-click track** — context menu (New Track, Rename, Delete)
- **Click track** — select (highlight active track)

---

## 4. Interaction Patterns

### 4.1 Keyboard Shortcuts

**Transport**
| Action | Shortcut |
|--------|---------|
| Play / Pause | `Space` |
| Stop (return to 0) | `Enter` |
| Arm recording | `R` |
| Loop toggle | `L` |

**Edit**
| Action | Shortcut |
|--------|---------|
| Undo | `Ctrl/Cmd + Z` |
| Redo | `Ctrl/Cmd + Y` or `Ctrl/Cmd + Shift + Z` |
| Cut | `Ctrl/Cmd + X` |
| Copy | `Ctrl/Cmd + C` |
| Paste | `Ctrl/Cmd + V` |
| Delete | `Del` or `Backspace` |
| Select all | `Ctrl/Cmd + A` |
| Duplicate | `Ctrl/Cmd + D` |

**File**
| Action | Shortcut |
|--------|---------|
| New project | `Ctrl/Cmd + N` |
| Open | `Ctrl/Cmd + O` |
| Save | `Ctrl/Cmd + S` |
| Save As | `Ctrl/Cmd + Shift + S` |
| Import audio | `Ctrl/Cmd + Shift + I` |
| Export WAV | `Ctrl/Cmd + Shift + E` |

**Tracks**
| Action | Shortcut |
|--------|---------|
| New track | `Ctrl/Cmd + Shift + T` |

**Navigation**
| Action | Shortcut |
|--------|---------|
| Zoom in timeline | `Ctrl/Cmd + Scroll` or `+` |
| Zoom out timeline | `Ctrl/Cmd + Scroll` or `-` |
| Zoom to fit | `Ctrl/Cmd + 0` |

[TODO: Full shortcut list to be defined and made configurable in Phase 5]

### 4.2 Drag & Drop

**Rules:**
- Drag is initiated after 3px of movement (prevents accidental triggers)
- A visual indicator (drop line) shows the destination before release
- Non-droppable targets visually reject the drag (`not-allowed` cursor)
- Undo applies to drag & drop

**Supported cases:**
- Clip → another track: move or copy (+ Alt)
- Audio file from OS → track: import
- Library assets → track / timeline: see [library-browser-ux.md](library-browser-ux.md) §5

### 4.3 Context Menus

Opened via right-click only.

**Rules:**
- Maximum 8 visible entries without a sub-menu
- No more than 1 level of sub-menu
- Destructive actions at the bottom, separated by a divider
- Keyboard shortcut displayed on the right of each entry

### 4.4 Tooltips

- Appearance delay: 500 ms (avoids visual noise)
- Position: always above or below, never obscuring the target
- Content: action label + keyboard shortcut if applicable
- Tooltips are required for icon-only buttons

---

## 5. Rules for UI Contributors

### Do

- Use existing SCSS variables — never hardcode values
- Test every component with keyboard navigation (Tab, visible focus)
- Check behaviour at different OS zoom levels (100%, 125%, 150%)
- Respect WCAG AA minimum contrast ratios (4.5:1 for text)
- Test on Windows AND macOS (fonts and WebView can differ)

### Do Not

- Hide information behind mandatory hovers (inaccessible on touchpad/tablet)
- Use animations > 300ms for frequent interactions
- Introduce a new interaction pattern without prior discussion
- Modify global SCSS variables without checking the impact on all components
- Add external UI dependencies without justification (keep the bundle lean)

### Contribution Process

1. Open an issue with a mockup (even rough) before writing code
2. Reference this document in the PR and check off the rules respected
3. Attach screenshots on Windows + macOS in the PR

---

## Links

- [ARCHITECTURE.md](ARCHITECTURE.md) — Svelte component structure
- [library-browser-ux.md](library-browser-ux.md) — Library wireframes, states, contexts, drop rules
- [library-browser-roadmap.md](library-browser-roadmap.md) — Library delivery phases
- [ADR-023](active/decisions/023-library-browser-ux.md) — Library UX decision freeze
- [CONTRIBUTING.md](../CONTRIBUTING.md) — General contribution process
- [TODO: Link to Figma/Penpot design system file]

# Rust `Self` rules (safe refactors)

Use when replacing `TypeName { … }` with `Self { … }` during **behaviour-preserving**
clean-up in `src-tauri/`. This is a **compile-time alias rule**, not a style fad:
wrong `Self` changes which type is built and breaks the build or silently builds
the wrong struct.

## What `Self` means

| Context                       | `Self` resolves to                         |
| ----------------------------- | ------------------------------------------ |
| `impl Foo { … }`              | `Foo`                                      |
| `impl Foo for Bar { … }`      | `Bar` (the type being implemented)         |
| Free function `fn f() -> Bar` | **Invalid** — no `Self` in scope (`E0411`) |
| Closure inside `impl Foo`     | `Foo`, **not** the closure’s return type   |

## Safe to use `Self`

- `impl T { fn new() -> Self { Self { … } } }` when the struct literal is **`T`**.
- `impl T { fn default() -> Self { Self { … } } }` on the same type.
- `Arc::new(Self { … })` inside `impl PluginInstance { … }` (returns `Arc<PluginInstance>`).
- `ModulationMap::new() -> Self::default()` when `#[derive(Default)]` on `ModulationMap`.
- Enum variant helpers inside `impl Enum { fn mk() -> Self { Self::Variant { … } } }`.

## Must use the explicit type name

1. **Return type ≠ implementing type**

   ```rust
   impl AppProject {
       pub fn to_dto(&self) -> ProjectDto {
           ProjectDto { … }  // not Self — Self would be AppProject
       }
   }
   ```

2. **Nested struct in a closure under the wrong `impl`**

   ```rust
   impl AppTrack {
       pub fn to_dto(&self) -> TrackDto {
           TrackDto {
               midi_clips: self.midi_clips.iter().map(|mc| MidiClipDto { … }).collect(),
               // not |mc| Self { … } — Self is TrackDto here
           }
       }
   }
   ```

3. **Free functions and test helpers**

   ```rust
   fn snapshot_project(p: &AppProject) -> AppProject {
       AppProject { … }  // not Self
   }

   fn pc(id: u32, v: f32) -> ParamChange {
       ParamChange { … }  // not Self
   }
   ```

4. **Private nested struct inside another type’s `impl`**

   ```rust
   impl PluginHost {
       fn insert(&mut self, …) {
           self.records.insert(key, DeviceRecord { … });
           // not Self { … } — Self is PluginHost
       }
   }
   ```

5. **`build()` on a builder that returns a different type**

   ```rust
   impl ExecutionPlanBuilder {
       fn build(self) -> ExecutionPlan {
           ExecutionPlan { … }  // not Self — Self is ExecutionPlanBuilder
       }
   }
   ```

## Quick check before replacing

1. Is the struct literal inside `impl X { … }`?
2. Is the literal building **exactly** `X` (same as the `impl` target)?
3. Is it **not** inside a closure whose block must produce another type?
4. Is it **not** a free `fn` / `#[cfg(test)]` helper?

If any answer is wrong → keep `ExplicitType { … }`.

## Project examples (real mistakes from safe-refactor passes)

| Location                               | Wrong                          | Right                         |
| -------------------------------------- | ------------------------------ | ----------------------------- |
| `impl AppProject::to_dto`              | `Self { name, … }`             | `ProjectDto { … }`            |
| `impl AppTrack::to_dto` + `map`        | `Self { id, … }` for MIDI clip | `MidiClipDto { … }`           |
| `fn snapshot_project`                  | —                              | `AppProject { … }` (free fn)  |
| `impl PluginHost` + `records.insert`   | `Self { instance, … }`         | `DeviceRecord { … }`          |
| `node_to_dto` / `route_to_dto`         | `Self { … }`                   | `GraphNodeDto` / `ModEdgeDto` |
| `#[cfg(test)] fn pc(…) -> ParamChange` | `Self { … }`                   | `ParamChange { … }`           |

## Allowed without `Self`

Keeping `ExplicitType { … }` when `Self` would be valid is **fine** — prefer
`Self` only when it removes noise and passes the checklist above.

## Validation

- `cargo build` / `cargo test` in `src-tauri/` after each batch.
- Grep for suspicious patterns: `-> OtherType` + body starts with `Self {`.

## Scope

- Applies to **safe refactors** (see `.claude/skills/safe-refactor/SKILL.md`).
- Does **not** change public API, serde shapes, or RT contracts.
- Not a license to rename types or move modules.

//! VST3 plugin format support (Windows): COM bindings (`ffi`), scanning
//! (`host`), the host context (`host_context`) and the objects it creates for a
//! plugin (`attribute_list`, `message`, with their reference count in
//! `com_object`), the track info sent to a controller (`channel_context`), the
//! controller's handler (`component_handler`), parameter
//! enumeration (`params`), the transport (`transport`), the events sent to an
//! instrument (`event_list`), the instrument adapter (`instrument`), its MIDI
//! controllers (`midi_mapping`), the per-block parameter changes
//! (`param_delivery`), the reader of `.vstpreset` files (`preset_file`) and the
//! graph node (`wrapper`).

pub mod channel_context;
pub mod component_handler;
#[cfg(all(test, windows))]
pub(crate) mod fake_plugin;
pub mod ffi;
// Moved to the `vst3_host` crate (ADR-033, step 3); the old paths still resolve.
pub use vst3_host::{buses, editor_view, event_list, host_context, midi_mapping, units};
pub mod host;
pub mod instrument;
pub mod param_delivery;
pub mod params;
pub mod preset_file;
pub mod preset_locations;
#[cfg(all(test, windows))]
pub(crate) mod test_dll;
#[cfg(all(test, windows))]
mod buses_tests;
#[cfg(all(test, windows))]
mod units_tests;
pub mod transport;
pub mod wrapper;

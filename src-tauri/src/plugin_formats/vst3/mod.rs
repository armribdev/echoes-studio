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

pub mod attribute_list;
pub mod channel_context;
mod com_object;
pub mod component_handler;
pub mod event_list;
#[cfg(all(test, windows))]
pub(crate) mod fake_plugin;
pub mod ffi;
pub mod host;
pub mod host_context;
pub mod instrument;
pub mod message;
pub mod midi_mapping;
pub mod param_delivery;
pub mod params;
pub mod preset_file;
pub mod transport;
pub mod wrapper;

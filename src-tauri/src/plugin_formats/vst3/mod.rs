//! VST3 plugin format support (Windows): COM bindings (`ffi`), scanning
//! (`host`), the host context (`host_context`) and the objects it creates for a
//! plugin (`attribute_list`, `message`, with their reference count in
//! `com_object`), the controller's handler (`component_handler`), parameter
//! enumeration (`params`), the transport (`transport`), the events sent to an
//! instrument (`event_list`), the instrument adapter (`instrument`), its MIDI
//! controllers (`midi_mapping`), the per-block parameter changes
//! (`param_delivery`) and the graph node (`wrapper`).

pub mod attribute_list;
mod com_object;
pub mod component_handler;
pub mod event_list;
pub mod ffi;
pub mod host;
pub mod host_context;
pub mod instrument;
pub mod message;
pub mod midi_mapping;
pub mod param_delivery;
pub mod params;
pub mod transport;
pub mod wrapper;

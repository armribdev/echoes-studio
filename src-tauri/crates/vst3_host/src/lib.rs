//! The host side of VST3 (ADR-033): the COM objects the host gives a plugin and
//! the per-block structures it fills for it. Built on the `vst3_ffi` bindings and
//! shared by the engine and the plugin bridge host. It knows nothing of the audio
//! graph, the project or the plugin instance of the engine.

mod com_object;

pub mod attribute_list;
pub mod buses;
pub mod channel_context;
pub mod component_handler;
pub mod editor_view;
pub mod event_list;
pub mod host_context;
pub mod message;
pub mod midi_mapping;
pub mod param_mirror;
pub mod params;
pub mod transport;
pub mod units;

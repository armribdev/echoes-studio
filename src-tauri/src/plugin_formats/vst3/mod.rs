//! VST3 plugin format support (Windows): COM bindings (`ffi`), scanning
//! (`host`), the host context (`host_context`), the controller's handler
//! (`component_handler`), parameter enumeration (`params`), the transport
//! (`transport`), the events sent to an instrument (`event_list`) and the graph
//! node (`wrapper`).

pub mod component_handler;
pub mod event_list;
pub mod ffi;
pub mod host;
pub mod host_context;
pub mod params;
pub mod transport;
pub mod wrapper;

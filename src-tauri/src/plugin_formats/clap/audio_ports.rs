//! CLAP output buses (ADR-014 multi-out).
//!
//! A plugin declares its output ports through `clap.audio-ports`.  The graph
//! exposes them as node output ports in *graph order*: port 0 is the main bus
//! (it flows down the track's effect chain), ports `1..` are the auxiliary
//! outputs another track can take as its input.  CLAP requires one
//! `clap_audio_buffer` per declared port, in the plugin's own index order, so
//! [`ClapOutputBuses`] keeps both orders.
//!
//! ## Real-time contract
//!
//! Everything is allocated in `new` / `prepare`; `bind` and `finish` only
//! write pre-allocated pointer tables and copy samples.

use std::ffi::CStr;

use crate::audio_graph::buffer::AudioBuffer;

use super::ffi::{
    ClapAudioBuffer, ClapAudioPortInfo, ClapPlugin, ClapPluginAudioPorts, CLAP_AUDIO_PORT_IS_MAIN,
    CLAP_EXT_AUDIO_PORTS,
};

/// One declared output port, as the plugin reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClapOutputPort {
    /// Position in the plugin's own `clap.audio-ports` output list.
    pub clap_index: u32,
    pub name: String,
    pub channel_count: u32,
}

/// Read the plugin's output ports in graph order (main first).  Empty when the
/// plugin has no `clap.audio-ports` extension or declares no output — callers
/// then keep the legacy single stereo output.
///
/// # Safety
/// `plugin` must be a live, `init`ed CLAP plugin, called from the main thread
/// while no `process` call is in flight.
pub unsafe fn read_output_ports(plugin: *const ClapPlugin) -> Vec<ClapOutputPort> {
    let ext = ((*plugin).get_extension)(plugin, CLAP_EXT_AUDIO_PORTS.as_ptr() as *const _);
    if ext.is_null() {
        return Vec::new();
    }
    let audio_ports = &*(ext as *const ClapPluginAudioPorts);
    let mut main = None;
    let mut aux = Vec::new();
    for clap_index in 0..(audio_ports.count)(plugin, false) {
        let mut info: ClapAudioPortInfo = std::mem::zeroed();
        if !(audio_ports.get)(plugin, clap_index, false, &mut info) {
            continue;
        }
        let port = ClapOutputPort {
            clap_index,
            name: port_name(&info),
            channel_count: info.channel_count,
        };
        if main.is_none() && info.flags & CLAP_AUDIO_PORT_IS_MAIN != 0 {
            main = Some(port);
        } else {
            aux.push(port);
        }
    }
    // No port flagged main: the plugin's first port plays that role.
    let main = main.or_else(|| (!aux.is_empty()).then(|| aux.remove(0)));
    main.into_iter().chain(aux).collect()
}

fn port_name(info: &ClapAudioPortInfo) -> String {
    CStr::from_bytes_until_nul(&info.name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Graph port → CLAP port mapping plus the per-block buffer tables handed to
/// `clap_process_t::audio_outputs`.
pub struct ClapOutputBuses {
    /// `(clap_index, channel_count)` per graph port.  Always at least one entry:
    /// a plugin without `clap.audio-ports` gets the legacy single main bus.
    ports: Vec<(usize, usize)>,
    channel_ptrs: Vec<Vec<*mut f32>>,
    buffers: Vec<ClapAudioBuffer>,
    /// Target for a port the scheduler did not provide a buffer for, so the
    /// plugin never receives a null channel pointer.
    spare: AudioBuffer,
    num_channels: usize,
}

impl ClapOutputBuses {
    pub fn new(ports: &[ClapOutputPort]) -> Self {
        let ports = if ports.is_empty() {
            vec![(0, 0)]
        } else {
            ports
                .iter()
                .map(|port| (port.clap_index as usize, port.channel_count as usize))
                .collect()
        };
        Self {
            ports,
            channel_ptrs: Vec::new(),
            buffers: Vec::new(),
            spare: AudioBuffer::silence(),
            num_channels: 0,
        }
    }

    /// Output ports the graph node exposes.
    pub fn num_ports(&self) -> usize {
        self.ports.len()
    }

    pub fn prepare(&mut self, num_channels: usize, max_block_size: usize) {
        self.num_channels = num_channels;
        self.spare = AudioBuffer::new(num_channels, max_block_size);
        let clap_ports = self.ports.iter().map(|&(index, _)| index + 1).max().unwrap_or(1);
        // Every CLAP port starts on the spare buffer, so one the plugin declared
        // but would not describe (`get` failed) still gets valid memory.
        let spare: Vec<*mut f32> = (0..num_channels)
            .map(|c| self.spare.channel_mut(c).as_mut_ptr())
            .collect();
        self.channel_ptrs = vec![spare; clap_ports];
        self.buffers = self
            .channel_ptrs
            .iter_mut()
            .map(|ptrs| ClapAudioBuffer {
                data32: ptrs.as_mut_ptr(),
                data64: std::ptr::null_mut(),
                channel_count: num_channels as u32,
                latency: 0,
                constant_mask: 0,
            })
            .collect();
    }

    /// Channels the plugin writes on graph port `port`: the engine's channel
    /// count on the main bus (legacy), the declared count (at most the
    /// engine's) on an aux bus.
    fn plugin_channels(&self, port: usize) -> usize {
        let declared = self.ports[port].1;
        if port == 0 || declared == 0 {
            self.num_channels
        } else {
            declared.min(self.num_channels)
        }
    }

    /// Point every CLAP output buffer at its graph output.  Returns the buffer
    /// table and its length, for `clap_process_t`.
    pub fn bind(&mut self, outputs: &mut [AudioBuffer]) -> (*mut ClapAudioBuffer, u32) {
        for port in 0..self.ports.len() {
            let clap_index = self.ports[port].0;
            let channels = self.plugin_channels(port);
            let target = match outputs.get_mut(port) {
                Some(buffer) => buffer,
                None => &mut self.spare,
            };
            for c in 0..channels {
                self.channel_ptrs[clap_index][c] = target.channel_mut(c).as_mut_ptr();
            }
            self.buffers[clap_index] = ClapAudioBuffer {
                data32: self.channel_ptrs[clap_index].as_mut_ptr(),
                data64: std::ptr::null_mut(),
                channel_count: channels as u32,
                latency: 0,
                constant_mask: 0,
            };
        }
        (self.buffers.as_mut_ptr(), self.buffers.len() as u32)
    }

    /// Spread a mono aux bus (typically a drum out) over every engine channel,
    /// so a receiving track hears it centred rather than left only.
    pub fn finish(&self, outputs: &mut [AudioBuffer]) {
        for (port, output) in outputs.iter_mut().enumerate().take(self.ports.len()) {
            if self.plugin_channels(port) == 1 {
                output.mono_to_all_channels();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(clap_index: u32, channel_count: u32) -> ClapOutputPort {
        ClapOutputPort {
            clap_index,
            name: String::new(),
            channel_count,
        }
    }

    #[test]
    fn no_declared_port_keeps_one_main_bus() {
        let mut buses = ClapOutputBuses::new(&[]);
        buses.prepare(2, 8);
        let mut outputs = vec![AudioBuffer::new(2, 8)];
        let (_, count) = buses.bind(&mut outputs);
        assert_eq!(buses.num_ports(), 1);
        assert_eq!(count, 1);
    }

    #[test]
    fn graph_ports_bind_to_their_clap_index() {
        // Main bus declared second by the plugin: graph port 0 must still be it.
        let mut buses = ClapOutputBuses::new(&[port(1, 2), port(0, 2)]);
        buses.prepare(2, 8);
        let mut outputs = vec![AudioBuffer::new(2, 8), AudioBuffer::new(2, 8)];
        let main_left = outputs[0].channel_mut(0).as_mut_ptr();
        let aux_left = outputs[1].channel_mut(0).as_mut_ptr();

        let (table, count) = buses.bind(&mut outputs);

        assert_eq!(count, 2);
        unsafe {
            assert_eq!(*(*table.add(1)).data32, main_left);
            assert_eq!(*(*table.add(0)).data32, aux_left);
        }
    }

    #[test]
    fn a_missing_graph_output_gets_a_spare_buffer_not_null() {
        let mut buses = ClapOutputBuses::new(&[port(0, 2), port(1, 2)]);
        buses.prepare(2, 8);
        let mut outputs = vec![AudioBuffer::new(2, 8)];

        let (table, _) = buses.bind(&mut outputs);

        unsafe {
            assert!(!(*(*table.add(1)).data32).is_null());
        }
    }

    #[test]
    fn an_undescribed_clap_port_still_gets_valid_memory() {
        // The plugin declared ports 0 and 2 but `get` failed for port 1.
        let mut buses = ClapOutputBuses::new(&[port(0, 2), port(2, 2)]);
        buses.prepare(2, 8);
        let mut outputs = vec![AudioBuffer::new(2, 8), AudioBuffer::new(2, 8)];

        let (table, count) = buses.bind(&mut outputs);

        assert_eq!(count, 3);
        unsafe {
            let gap = &*table.add(1);
            assert_eq!(gap.channel_count, 2);
            assert!(!(*gap.data32).is_null() && !(*gap.data32.add(1)).is_null());
        }
    }

    #[test]
    fn a_mono_aux_bus_is_spread_over_both_channels() {
        let mut buses = ClapOutputBuses::new(&[port(0, 2), port(1, 1)]);
        buses.prepare(2, 8);
        let mut outputs = vec![AudioBuffer::new(2, 8), AudioBuffer::new(2, 8)];
        let (table, _) = buses.bind(&mut outputs);
        assert_eq!(unsafe { (*table.add(1)).channel_count }, 1);

        outputs[1].channel_mut(0).fill(0.25);
        buses.finish(&mut outputs);

        assert!(outputs[1].channel(1).iter().all(|&s| s == 0.25));
    }
}

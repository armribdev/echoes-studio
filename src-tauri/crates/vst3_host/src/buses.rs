//! Audio buses of a VST3 plugin (ADR-014 multi-out, VST3 catch-up step 4e).
//!
//! A component declares any number of audio output buses; graph output port `p`
//! is output bus `p`, the main bus being port 0.  The plugin is third-party
//! code, so every count it reports is bounded before it drives a loop.

use std::ffi::c_void;
use std::mem::{offset_of, size_of};

use audio_buffer::AudioBuffer;
use vst3_ffi::{
    string128_to_string, AudioBusBuffers, IAudioProcessor, IComponent, String128, K_AUDIO, K_INPUT,
    K_OUTPUT, K_RESULT_OK, SPEAKER_STEREO,
};

/// Audio buses read per direction.  The set of output buses a host asks for is a
/// `u32` mask, so no more than 32 can be named.
pub const MAX_AUDIO_BUSES: i32 = 32;

/// Every output bus but the main one (bus 0, which is always open).
pub const AUX_OUTPUT_BUSES: u32 = !1;

/// Channels one bus may declare; a 7th order ambisonic bus has 64.
const MAX_BUS_CHANNELS: i32 = 64;

/// `BusInfo::BusFlags::kIsControlVoltage` (`ivstcomponent.h`).
const BUS_IS_CONTROL_VOLTAGE: u32 = 1 << 1;

/// `BusTypes::kAux` (`ivstcomponent.h`).
const BUS_TYPE_AUX: i32 = 1;

/// `Vst::BusInfo` (`ivstcomponent.h`).
#[repr(C)]
pub struct BusInfo {
    pub media_type: i32,
    pub direction: i32,
    pub channel_count: i32,
    pub name: String128,
    pub bus_type: i32,
    pub flags: u32,
}

const _: () = {
    assert!(size_of::<BusInfo>() == 276, "BusInfo layout drift");
    assert!(offset_of!(BusInfo, bus_type) == 268, "BusInfo layout drift");
};

/// One audio output bus, as the plugin describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vst3OutputBus {
    pub name: String,
    pub channel_count: u32,
    /// Carries control data at sample rate, not sound: never routed to a track.
    pub control_voltage: bool,
    /// Opened by the host at the last activation; an open bus gets its own
    /// sound, a closed one is mixed by the plugin into its main bus.
    pub active: bool,
}

/// The audio output buses of `component`, in bus order.  Reading stops at the
/// first bus the plugin will not describe, so a graph port is always the bus of
/// the same index.
///
/// # Safety
/// `component` must be a live, initialised `IComponent`, called off the audio
/// thread while no `process` call is in flight.
pub unsafe fn read_output_buses(component: *mut IComponent) -> Vec<Vst3OutputBus> {
    describe_buses(component, K_OUTPUT)
        .into_iter()
        .map(|info| Vst3OutputBus {
            name: string128_to_string(&info.name),
            channel_count: info.channel_count.clamp(0, MAX_BUS_CHANNELS) as u32,
            control_voltage: info.flags & BUS_IS_CONTROL_VOLTAGE != 0,
            active: false,
        })
        .collect()
}

/// One auxiliary audio input bus (a sidechain).  Graph input port `p` is input
/// bus `p`, the main bus being port 0, so the first auxiliary input is port 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vst3AuxInput {
    pub channel_count: u32,
}

/// The auxiliary audio input buses of `component`: the buses after the main one
/// that are of type `kAux`, up to the first that is not (the SDK puts every
/// `kMain` bus before the `kAux` ones).  A control-voltage input is not sound,
/// so it ends the run too.
///
/// # Safety
/// As [`read_output_buses`].
pub unsafe fn read_aux_inputs(component: *mut IComponent) -> Vec<Vst3AuxInput> {
    describe_buses(component, K_INPUT)
        .into_iter()
        .skip(1)
        .take_while(|info| {
            info.bus_type == BUS_TYPE_AUX && info.flags & BUS_IS_CONTROL_VOLTAGE == 0
        })
        .map(|info| Vst3AuxInput {
            channel_count: info.channel_count.clamp(0, MAX_BUS_CHANNELS) as u32,
        })
        .collect()
}

/// The audio buses of one direction as the plugin describes them, up to the
/// first one it will not describe.
unsafe fn describe_buses(component: *mut IComponent, direction: i32) -> Vec<BusInfo> {
    let vtbl = &*(*component).vtbl;
    let declared = (vtbl.get_bus_count)(component, K_AUDIO, direction).clamp(0, MAX_AUDIO_BUSES);
    (0..declared)
        .map_while(|index| {
            let mut info: BusInfo = std::mem::zeroed();
            let described = (vtbl.get_bus_info)(
                component,
                K_AUDIO,
                direction,
                index,
                &mut info as *mut BusInfo as *mut c_void,
            );
            (described == K_RESULT_OK).then_some(info)
        })
        .collect()
}

/// Opens the main output bus, and the auxiliary ones `wanted` names (bit `n` =
/// bus `n`) unless they carry control voltage.  A bus opened by an earlier
/// activation (`previous`) and no longer wanted is closed, because a bus stays
/// open until the host says otherwise.  Marks each bus of `buses` accordingly.
///
/// # Safety
/// As [`read_output_buses`], with the plugin not active.
pub unsafe fn open_output_buses(
    component: *mut IComponent,
    buses: &mut [Vst3OutputBus],
    wanted: u32,
    previous: u32,
) {
    let activate_bus = (*(*component).vtbl).activate_bus;
    activate_bus(component, K_AUDIO, K_OUTPUT, 0, 1);
    for (index, bus) in buses.iter_mut().enumerate() {
        let bit = 1u32 << index;
        bus.active = index == 0 || (wanted & bit != 0 && !bus.control_voltage);
        if index == 0 {
            continue;
        }
        if bus.active {
            activate_bus(component, K_AUDIO, K_OUTPUT, index as i32, 1);
        } else if previous & bit != 0 {
            activate_bus(component, K_AUDIO, K_OUTPUT, index as i32, 0);
        }
    }
}

/// Tells the plugin the arrangement of every audio bus it declared, as
/// `setBusArrangements` requires (one per bus, inputs and outputs).  The main bus
/// of each direction is asked for stereo; any other bus is offered the
/// arrangement it already has, so a mono or surround bus is not forced.  The
/// plugin may answer that it prefers another layout: the buses are read again
/// afterwards, so its answer is not needed here.
///
/// Returns whether the plugin has an audio input bus.
///
/// # Safety
/// As [`read_output_buses`], with `processor` the same plugin's `IAudioProcessor`,
/// before the plugin is activated.
pub unsafe fn propose_arrangements(
    component: *mut IComponent,
    processor: *mut IAudioProcessor,
) -> bool {
    let mut inputs = proposed_arrangements(component, processor, K_INPUT);
    let mut outputs = proposed_arrangements(component, processor, K_OUTPUT);
    ((*(*processor).vtbl).set_bus_arrangements)(
        processor,
        arrangements_ptr(&mut inputs),
        inputs.len() as i32,
        arrangements_ptr(&mut outputs),
        outputs.len() as i32,
    );
    !inputs.is_empty()
}

unsafe fn proposed_arrangements(
    component: *mut IComponent,
    processor: *mut IAudioProcessor,
    direction: i32,
) -> Vec<u64> {
    let declared = ((*(*component).vtbl).get_bus_count)(component, K_AUDIO, direction)
        .clamp(0, MAX_AUDIO_BUSES);
    (0..declared)
        .map(|index| match index {
            0 => SPEAKER_STEREO,
            _ => current_arrangement(processor, direction, index),
        })
        .collect()
}

/// The arrangement a bus has now; stereo when the plugin will not say.
unsafe fn current_arrangement(processor: *mut IAudioProcessor, direction: i32, index: i32) -> u64 {
    let mut arrangement = 0;
    let answered =
        ((*(*processor).vtbl).get_bus_arrangement)(processor, direction, index, &mut arrangement);
    if answered == K_RESULT_OK {
        arrangement
    } else {
        SPEAKER_STEREO
    }
}

/// A plugin with no bus in a direction is given a null array, not a dangling one.
fn arrangements_ptr(arrangements: &mut [u64]) -> *mut u64 {
    if arrangements.is_empty() {
        std::ptr::null_mut()
    } else {
        arrangements.as_mut_ptr()
    }
}

/// One graph output port and the bus behind it.
struct OutputSlot {
    /// Channels the bus declared; 0 when the plugin did not say.
    declared: usize,
    open: bool,
}

/// Graph port → bus mapping plus the per-block tables handed to
/// `ProcessData::outputs` (ADR-014 multi-out), shared by the effect and the
/// instrument.  Port `p` is output bus `p`; the main bus keeps the engine's
/// channel count, an auxiliary bus its declared one.
///
/// ## Real-time contract
///
/// Everything is allocated in `prepare`; `bind` and `finish` only write the
/// pointer tables and copy samples.
pub struct Vst3OutputBuses {
    slots: Vec<OutputSlot>,
    /// Sample pointers of each bus: the graph buffer's channels, or `spare` for a
    /// channel the graph has no room for and for a closed bus.
    channel_ptrs: Vec<Vec<*mut f32>>,
    /// The `AudioBusBuffers` of every bus up to the last open one.
    table: Vec<AudioBusBuffers>,
    spare: AudioBuffer,
    num_channels: usize,
}

impl Vst3OutputBuses {
    /// A plugin whose buses are not known (none read yet, or none declared) has
    /// the single main bus, as before multi-out.
    pub fn new(buses: &[Vst3OutputBus]) -> Self {
        let slots = if buses.is_empty() {
            vec![OutputSlot {
                declared: 0,
                open: true,
            }]
        } else {
            buses
                .iter()
                .map(|bus| OutputSlot {
                    declared: bus.channel_count as usize,
                    open: bus.active,
                })
                .collect()
        };
        Vst3OutputBuses {
            slots,
            channel_ptrs: Vec::new(),
            table: Vec::new(),
            spare: AudioBuffer::silence(),
            num_channels: 0,
        }
    }

    /// Output ports the graph node exposes: every declared bus, open or not.
    pub fn num_ports(&self) -> usize {
        self.slots.len()
    }

    pub fn prepare(&mut self, num_channels: usize, max_block_size: usize) {
        self.num_channels = num_channels;
        self.spare = AudioBuffer::new(num_channels, max_block_size);
        let spare = self.spare.channel_mut(0).as_mut_ptr();
        self.channel_ptrs = (0..self.slots.len())
            .map(|port| vec![spare; self.plugin_channels(port)])
            .collect();
        let open_buses = self
            .slots
            .iter()
            .rposition(|slot| slot.open)
            .map_or(1, |last| last + 1);
        let mut table = Vec::with_capacity(open_buses);
        for port in 0..open_buses {
            table.push(AudioBusBuffers {
                num_channels: self.plugin_channels(port) as i32,
                silence_flags: 0,
                channel_buffers32: self.channel_ptrs[port].as_mut_ptr(),
            });
        }
        self.table = table;
    }

    /// Channels the plugin writes on port `port`; an auxiliary bus may have more
    /// than the engine, the plugin being told the layout it declared.
    fn plugin_channels(&self, port: usize) -> usize {
        match self.slots[port].declared {
            declared if port == 0 || declared == 0 => self.num_channels,
            declared => declared,
        }
    }

    /// Point every open bus at its graph output.  Returns the table and its
    /// length, for `ProcessData::outputs` / `numOutputs`: the buses up to the last
    /// open one, so a closed bus between two open ones is still a valid entry.
    pub fn bind(&mut self, outputs: &mut [AudioBuffer]) -> (*mut AudioBusBuffers, i32) {
        for port in 0..self.table.len() {
            if !self.slots[port].open {
                continue;
            }
            let bound = self.plugin_channels(port).min(self.num_channels);
            let target = match outputs.get_mut(port) {
                Some(buffer) => buffer,
                None => &mut self.spare,
            };
            for channel in 0..bound {
                self.channel_ptrs[port][channel] = target.channel_mut(channel).as_mut_ptr();
            }
        }
        (self.table.as_mut_ptr(), self.table.len() as i32)
    }

    /// Spread a mono auxiliary bus over every engine channel, so a receiving
    /// track hears it centred rather than left only.
    pub fn finish(&self, outputs: &mut [AudioBuffer]) {
        for port in 1..self.table.len().min(outputs.len()) {
            if self.slots[port].open && self.plugin_channels(port) == 1 {
                outputs[port].mono_to_all_channels();
            }
        }
    }
}

/// The bus table handed to `ProcessData::inputs` (ADR-014 sidechain), shared by
/// the effect wrappers: the main bus, then every auxiliary bus the plugin opened.
/// Input port `p` is input bus `p`.
///
/// An auxiliary bus the graph does not feed is given silence, with its silence
/// flags set; one that is fed gets the channels of its source, a mono mix when
/// the plugin wants a single channel, and a silence flag only on the channels
/// that are exactly zero.  The plugin never gets a null channel pointer for an
/// auxiliary bus.
///
/// ## Real-time contract
///
/// Everything is allocated in `prepare`; `bind` only writes pointer tables, the
/// mono mix and the silence flags.
pub struct Vst3InputBuses {
    /// Channels each auxiliary bus declared; 0 when the plugin did not say.
    aux_declared: Vec<usize>,
    /// Sample pointers of each bus, the main bus first.
    channel_ptrs: Vec<Vec<*mut f32>>,
    table: Vec<AudioBusBuffers>,
    /// Channel `k` is the mono mix handed to auxiliary bus `k`.
    mono_mix: AudioBuffer,
    /// Read-only zeros standing in for a missing source or channel.
    silent: AudioBuffer,
    num_channels: usize,
}

impl Vst3InputBuses {
    /// A plugin with no auxiliary input has the single main bus, as before the
    /// sidechain.
    pub fn new(aux_inputs: &[Vst3AuxInput]) -> Self {
        Vst3InputBuses {
            aux_declared: aux_inputs
                .iter()
                .map(|bus| bus.channel_count as usize)
                .collect(),
            channel_ptrs: Vec::new(),
            table: Vec::new(),
            mono_mix: AudioBuffer::silence(),
            silent: AudioBuffer::silence(),
            num_channels: 0,
        }
    }

    /// Input ports the graph node exposes: the main bus and each auxiliary one.
    pub fn num_ports(&self) -> usize {
        1 + self.aux_declared.len()
    }

    pub fn prepare(&mut self, num_channels: usize, max_block_size: usize) {
        self.num_channels = num_channels;
        self.mono_mix = AudioBuffer::new(self.aux_declared.len().max(1), max_block_size);
        self.silent = AudioBuffer::new(1, max_block_size);
        let silent = self.silent.channel(0).as_ptr() as *mut f32;
        self.channel_ptrs = (0..self.num_ports())
            .map(|bus| match bus {
                0 => vec![std::ptr::null_mut(); num_channels],
                aux => vec![silent; self.plugin_channels(aux - 1)],
            })
            .collect();
        self.table = self
            .channel_ptrs
            .iter_mut()
            .map(|pointers| AudioBusBuffers {
                num_channels: pointers.len() as i32,
                silence_flags: 0,
                channel_buffers32: pointers.as_mut_ptr(),
            })
            .collect();
    }

    /// Channels the plugin reads on auxiliary bus `aux`.
    fn plugin_channels(&self, aux: usize) -> usize {
        match self.aux_declared[aux] {
            0 => self.num_channels,
            declared => declared,
        }
    }

    /// Point every bus at the graph inputs of its port (`inputs[0]` is the main
    /// bus, `inputs[1..]` the auxiliary ones, possibly fewer than the plugin has).
    /// Returns the table and its length, for `ProcessData::inputs` / `numInputs`.
    /// `frames` is the real block length, at most the buffers' capacity.
    pub fn bind(&mut self, inputs: &[&AudioBuffer], frames: usize) -> (*mut AudioBusBuffers, i32) {
        let frames = frames.min(self.silent.num_frames);
        match inputs.first() {
            Some(main) => {
                for (channel, pointer) in self.channel_ptrs[0].iter_mut().enumerate() {
                    *pointer = main.channel(channel).as_ptr() as *mut f32;
                }
                self.table[0].silence_flags = 0;
            }
            None => {
                self.channel_ptrs[0].fill(std::ptr::null_mut());
                self.table[0].silence_flags = u64::MAX;
            }
        }
        for aux in 0..self.aux_declared.len() {
            let source = inputs.get(1 + aux).copied();
            self.bind_aux(aux, source, frames);
        }
        (self.table.as_mut_ptr(), self.table.len() as i32)
    }

    fn bind_aux(&mut self, aux: usize, source: Option<&AudioBuffer>, frames: usize) {
        let wanted = self.plugin_channels(aux);
        let mixed = match source {
            Some(source) if wanted == 1 && source.num_channels > 1 => {
                self.mix_to_mono(aux, source, frames);
                true
            }
            _ => false,
        };
        let mut silence_flags = 0u64;
        for channel in 0..wanted {
            let samples: &[f32] = match source {
                Some(_) if mixed => self.mono_mix.channel(aux),
                Some(source) if channel < source.num_channels => source.channel(channel),
                _ => self.silent.channel(0),
            };
            self.channel_ptrs[1 + aux][channel] = samples.as_ptr() as *mut f32;
            if samples[..frames].iter().all(|&sample| sample == 0.0) {
                silence_flags |= 1 << channel;
            }
        }
        self.table[1 + aux].silence_flags = silence_flags;
    }

    /// The average of every channel of `source`, in the mix channel of `aux`.
    fn mix_to_mono(&mut self, aux: usize, source: &AudioBuffer, frames: usize) {
        let mix = &mut self.mono_mix.channel_mut(aux)[..frames];
        mix.copy_from_slice(&source.channel(0)[..frames]);
        for channel in 1..source.num_channels {
            for (mixed, &sample) in mix.iter_mut().zip(&source.channel(channel)[..frames]) {
                *mixed += sample;
            }
        }
        let gain = 1.0 / source.num_channels as f32;
        for mixed in mix.iter_mut() {
            *mixed *= gain;
        }
    }
}

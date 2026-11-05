//! Process lifecycle and audio transport of the out-of-process bridge (ADR-015).
//!
//! ## Shared region
//!
//! ```text
//!   [control block 16 B][audio-in ring][audio-out ring]
//!   control block: magic:u32 | protocol:u32 | region_bytes:u32 | status:AtomicU32
//! ```
//!
//! The engine ([`BridgeProcess`], owner) creates the region, spawns
//! `plugin_host_bridge`, and waits for the child ([`BridgePeer`]) to report
//! `READY` — which it does only once its plugin is loaded.  Dropping the
//! process sets `SHUTDOWN` and reaps the child.
//!
//! ## One audio block
//!
//! [`AudioPort::process_block`] (engine audio thread) writes the block (header,
//! typed sections, samples — see [`crate::protocol`]) into the audio-in ring,
//! signals the wake event, and spins — bounded by a budget — for the reply with
//! the same sequence.  The bridge's audio thread
//! ([`BridgePeer::serve_audio`]) sleeps on that event, processes the block, and
//! pushes the reply.  A block that misses the budget is silence; after
//! [`DEAD_AFTER_CONSECUTIVE_TIMEOUTS`] such blocks in a row the port is dead and
//! answers silence at once (a crashed or hung plugin stops costing the audio
//! thread anything).

use std::cell::UnsafeCell;
use std::ffi::OsString;
use std::io::{self, BufReader};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use crate::control::{self, Op, STATUS_OK};
use crate::protocol::{
    read_samples, write_records, write_samples, BlockHeader, BlockShape, Section, WireNote,
    WireParam, WireRecord, WireTransport, BLOCK_HAS_INPUT, BLOCK_HAS_TRANSPORT, BLOCK_HEADER_BYTES,
    BLOCK_RESET, BRIDGE_PROTOCOL_VERSION, NOTE_BYTES, PARAM_BYTES, TRANSPORT_BYTES,
};
use crate::region::{SharedEvent, SharedRegion};
use crate::shmem::{RingLayout, ShmemRing};

/// ASCII `"EsCt"` (Echoes Control), little-endian.
pub const CONTROL_MAGIC: u32 = u32::from_le_bytes(*b"EsCt");

pub const STATUS_INIT: u32 = 0;
pub const STATUS_READY: u32 = 1;
pub const STATUS_SHUTDOWN: u32 = 2;
pub const STATUS_ERROR: u32 = 3;

const OFF_MAGIC: usize = 0;
const OFF_PROTOCOL: usize = 4;
const OFF_REGION_BYTES: usize = 8;
const OFF_STATUS: usize = 12;

/// Bytes of the control block at the head of the region.
pub const CONTROL_BYTES: usize = 16;

/// Messages each audio ring can hold.  A synchronous round-trip needs one; the
/// rest absorbs a reply that arrives after its block timed out.
const RING_SLOTS: usize = 4;

/// Consecutive timed-out blocks after which the port is declared dead.  A
/// one-off scheduling hiccup does not repeat eight blocks in a row.
pub const DEAD_AFTER_CONSECUTIVE_TIMEOUTS: u32 = 8;

/// How long the bridge's audio thread sleeps on the wake event before calling
/// [`BridgeProcessor::idle`].
const IDLE_WAIT_MS: u32 = 10;

// ── Layout ──────────────────────────────────────────────────────────────────

/// Shape of a device's shared region; engine and bridge must agree on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BridgeLayout {
    /// Largest block the rings can carry.
    pub max_frames: u32,
    pub channels: u32,
    /// Bytes reserved per block for events, in each direction.
    pub event_capacity: u32,
}

impl BridgeLayout {
    pub fn new(max_frames: u32, channels: u32, event_capacity: u32) -> Self {
        BridgeLayout {
            max_frames,
            channels,
            event_capacity,
        }
    }

    /// Samples in the largest block (all channels).
    pub fn max_samples(&self) -> usize {
        self.max_frames as usize * self.channels as usize
    }

    /// Largest ring message: header + events + samples.
    pub fn slot_size(&self) -> usize {
        BLOCK_HEADER_BYTES + self.event_capacity as usize + self.max_samples() * 4
    }

    fn ring(&self) -> RingLayout {
        RingLayout::new(RING_SLOTS, self.slot_size())
    }

    fn ring_bytes(&self) -> usize {
        self.ring().bytes_required()
    }

    fn audio_in_offset(&self) -> usize {
        CONTROL_BYTES
    }

    fn audio_out_offset(&self) -> usize {
        CONTROL_BYTES + self.ring_bytes()
    }

    pub fn region_bytes(&self) -> usize {
        CONTROL_BYTES + 2 * self.ring_bytes()
    }

    /// Command-line arguments that pass this layout to the bridge.
    pub fn to_args(&self) -> Vec<OsString> {
        [
            ("--max-frames", self.max_frames),
            ("--channels", self.channels),
            ("--event-capacity", self.event_capacity),
        ]
        .into_iter()
        .flat_map(|(flag, value)| [OsString::from(flag), OsString::from(value.to_string())])
        .collect()
    }
}

// ── Control block ───────────────────────────────────────────────────────────

/// # Safety
/// `base` must point to at least [`CONTROL_BYTES`] writable, 4-byte-aligned bytes.
unsafe fn write_control_init(base: *mut u8, region_bytes: u32) {
    core::ptr::write_unaligned(base.add(OFF_MAGIC) as *mut u32, CONTROL_MAGIC);
    core::ptr::write_unaligned(base.add(OFF_PROTOCOL) as *mut u32, BRIDGE_PROTOCOL_VERSION);
    core::ptr::write_unaligned(base.add(OFF_REGION_BYTES) as *mut u32, region_bytes);
    status_cell(base).store(STATUS_INIT, Ordering::Release);
}

/// `(magic, protocol, region_bytes)`.
///
/// # Safety
/// Same as [`write_control_init`]; the header must be initialised.
unsafe fn read_control(base: *mut u8) -> (u32, u32, u32) {
    (
        core::ptr::read_unaligned(base.add(OFF_MAGIC) as *const u32),
        core::ptr::read_unaligned(base.add(OFF_PROTOCOL) as *const u32),
        core::ptr::read_unaligned(base.add(OFF_REGION_BYTES) as *const u32),
    )
}

/// # Safety
/// Same as [`write_control_init`].
unsafe fn status_cell<'a>(base: *mut u8) -> &'a AtomicU32 {
    &*(base.add(OFF_STATUS) as *const AtomicU32)
}

fn unique_region_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "echoes_bridge_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn wake_event_name(region_name: &str) -> String {
    format!("{region_name}_wake")
}

// ── Engine side: audio port ─────────────────────────────────────────────────

/// One block handed to the bridge.
pub struct BlockRequest<'a> {
    pub frames: usize,
    /// Planar input (`input_channels × frames`), or `None` for a generator.
    pub input: Option<&'a [f32]>,
    /// Channels of every input port of the block.
    pub input_channels: usize,
    /// Channels the reply carries: every output port of the block.
    pub output_channels: usize,
    pub transport: Option<&'a WireTransport>,
    /// Notes and raw MIDI, in the order the plugin must see them.
    pub notes: &'a [WireNote],
    pub params: &'a [WireParam],
    pub sequence: u64,
    pub steady_time: i64,
    /// Clear the plugin's DSP state before processing.
    pub reset: bool,
}

impl BlockRequest<'_> {
    /// Bytes the sections take in the ring message.
    fn section_bytes(&self) -> usize {
        self.transport.map_or(0, |_| TRANSPORT_BYTES)
            + self.notes.len() * NOTE_BYTES
            + self.params.len() * PARAM_BYTES
    }
}

/// Buffers the caller lends for the answer, so nothing is allocated per block.
pub struct BlockReply<'a> {
    /// Planar output (`output_channels × frames` samples used).
    pub output: &'a mut [f32],
    /// What the plugin did to its own parameters (edits, gestures).
    pub edits: &'a mut [WireParam],
    /// The MIDI the plugin produced.
    pub midi: &'a mut [WireNote],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOutcome {
    /// The bridge answered: `output` holds the processed block, the first
    /// `edit_count` of `edits` and `midi_count` of `midi` what the plugin sent back.
    Processed {
        flags: u32,
        edit_count: usize,
        midi_count: usize,
    },
    /// No answer within the budget: `output` is silence.
    TimedOut,
    /// The watchdog condemned the port: `output` is silence.
    Dead,
    /// Not sent — another caller holds the port, or the block does not fit the
    /// layout.  `output` is untouched; the caller decides what to play.
    Skipped,
}

struct PortScratch {
    send: Vec<u8>,
    recv: Vec<u8>,
}

/// Engine end of the audio rings.  Real-time safe: no allocation, no lock, no
/// wait beyond the caller's budget; the only kernel call is the non-blocking
/// wake signal.
pub struct AudioPort {
    layout: BridgeLayout,
    audio_in: ShmemRing,
    audio_out: ShmemRing,
    wake: SharedEvent,
    /// Non-blocking try-lock: one block in flight at a time (the live graph and
    /// an offline render may share a device).
    busy: AtomicBool,
    scratch: UnsafeCell<PortScratch>,
    timeout_count: AtomicU64,
    consecutive_timeouts: AtomicU32,
    dead: AtomicBool,
}

// SAFETY: the rings' producer/consumer ends and the scratch buffers are only
// touched by the caller that won `busy`, so they are never used concurrently.
unsafe impl Send for AudioPort {}
unsafe impl Sync for AudioPort {}

impl AudioPort {
    /// Owner side: stamp the control block, initialise both rings in `region`
    /// and create the wake event.
    fn create(region: &SharedRegion, region_name: &str, layout: BridgeLayout) -> io::Result<Self> {
        let base = region.as_mut_ptr();
        let ring = layout.ring();
        let bytes = ring.bytes_required();
        // SAFETY: the region was sized from `layout` and is page-aligned.
        let (audio_in, audio_out) = unsafe {
            write_control_init(base, layout.region_bytes() as u32);
            (
                ShmemRing::init(base.add(layout.audio_in_offset()), bytes, ring),
                ShmemRing::init(base.add(layout.audio_out_offset()), bytes, ring),
            )
        };
        let slot = layout.slot_size();
        Ok(AudioPort {
            layout,
            audio_in,
            audio_out,
            wake: SharedEvent::create(&wake_event_name(region_name))?,
            busy: AtomicBool::new(false),
            scratch: UnsafeCell::new(PortScratch {
                send: vec![0; slot],
                recv: vec![0; slot],
            }),
            timeout_count: AtomicU64::new(0),
            consecutive_timeouts: AtomicU32::new(0),
            dead: AtomicBool::new(false),
        })
    }

    pub fn layout(&self) -> &BridgeLayout {
        &self.layout
    }

    /// Send one block and wait, at most `budget`, for its reply.
    pub fn process_block(
        &self,
        request: &BlockRequest,
        reply: &mut BlockReply,
        budget: Duration,
    ) -> BlockOutcome {
        let input_samples = request.frames * request.input_channels;
        let output_samples = request.frames * request.output_channels;
        let capacity = self.layout.channels as usize;
        let fits = request.frames <= self.layout.max_frames as usize
            && request.input_channels <= capacity
            && request.output_channels <= capacity
            && reply.output.len() >= output_samples
            && request.section_bytes() <= self.layout.event_capacity as usize
            && request.input.is_none_or(|input| input.len() >= input_samples);
        if !fits {
            return BlockOutcome::Skipped;
        }
        if self.dead.load(Ordering::Relaxed) {
            reply.output[..output_samples].fill(0.0);
            return BlockOutcome::Dead;
        }
        if self.busy.swap(true, Ordering::Acquire) {
            return BlockOutcome::Skipped;
        }
        // SAFETY: `busy` grants exclusive use of the scratch buffers.
        let scratch = unsafe { &mut *self.scratch.get() };
        let outcome = self.round_trip(request, reply, budget, scratch);
        self.busy.store(false, Ordering::Release);
        self.note_outcome(outcome);
        outcome
    }

    fn round_trip(
        &self,
        request: &BlockRequest,
        reply: &mut BlockReply,
        budget: Duration,
        scratch: &mut PortScratch,
    ) -> BlockOutcome {
        let input_samples = request.frames * request.input_channels;
        let output_samples = request.frames * request.output_channels;
        // Replies to blocks that already timed out.
        while self.audio_out.pop(&mut scratch.recv).is_some() {}

        let mut flags = 0;
        if request.input.is_some() {
            flags |= BLOCK_HAS_INPUT;
        }
        if request.reset {
            flags |= BLOCK_RESET;
        }
        if request.transport.is_some() {
            flags |= BLOCK_HAS_TRANSPORT;
        }
        let header = BlockHeader::new(BlockShape {
            frames: request.frames as u32,
            channel_count: request.input_channels as u32,
            flags,
            note_count: request.notes.len() as u32,
            param_count: request.params.len() as u32,
            sequence: request.sequence,
            steady_time: request.steady_time,
        });
        header.write(&mut scratch.send);
        let mut len = BLOCK_HEADER_BYTES;
        if let Some(transport) = request.transport {
            transport.write(&mut scratch.send[len..]);
            len += TRANSPORT_BYTES;
        }
        len = write_records(request.notes, &mut scratch.send, len);
        len = write_records(request.params, &mut scratch.send, len);
        if let Some(input) = request.input {
            let end = len + input_samples * 4;
            write_samples(&input[..input_samples], &mut scratch.send[len..end]);
            len = end;
        }
        if !self.audio_in.push(&scratch.send[..len]) {
            reply.output[..output_samples].fill(0.0);
            return BlockOutcome::TimedOut;
        }
        self.wake.signal();

        let deadline = Instant::now() + budget;
        loop {
            if let Some(received) = self.audio_out.pop(&mut scratch.recv) {
                let message = &scratch.recv[..received];
                match BlockHeader::from_bytes(message) {
                    Some(answer) if answer.sequence == request.sequence => {
                        return read_reply(&answer, message, output_samples, reply);
                    }
                    _ => continue,
                }
            }
            if Instant::now() >= deadline {
                reply.output[..output_samples].fill(0.0);
                return BlockOutcome::TimedOut;
            }
            std::hint::spin_loop();
        }
    }

    fn note_outcome(&self, outcome: BlockOutcome) {
        match outcome {
            BlockOutcome::Processed { .. } => self.consecutive_timeouts.store(0, Ordering::Relaxed),
            BlockOutcome::TimedOut => {
                self.timeout_count.fetch_add(1, Ordering::Relaxed);
                let run = self.consecutive_timeouts.fetch_add(1, Ordering::Relaxed) + 1;
                if run >= DEAD_AFTER_CONSECUTIVE_TIMEOUTS {
                    self.dead.store(true, Ordering::Relaxed);
                }
            }
            BlockOutcome::Dead | BlockOutcome::Skipped => {}
        }
    }

    /// Blocks that missed their budget since spawn.
    pub fn timeout_count(&self) -> u64 {
        self.timeout_count.load(Ordering::Relaxed)
    }

    /// Whether the watchdog condemned the port.  Final for this process.
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Relaxed)
    }
}

fn read_reply(
    answer: &BlockHeader,
    message: &[u8],
    samples: usize,
    reply: &mut BlockReply,
) -> BlockOutcome {
    let notes_at = (BLOCK_HEADER_BYTES + answer.transport_bytes()).min(message.len());
    let params_at = (notes_at + answer.note_count as usize * NOTE_BYTES).min(message.len());
    let samples_at = (params_at + answer.param_count as usize * PARAM_BYTES).min(message.len());

    let midi = Section::<WireNote>::new(&message[notes_at..], answer.note_count as usize);
    let midi_count = midi.len().min(reply.midi.len());
    for (slot, note) in reply.midi.iter_mut().zip(midi.iter()) {
        *slot = note;
    }
    let edits = Section::<WireParam>::new(&message[params_at..], answer.param_count as usize);
    let edit_count = edits.len().min(reply.edits.len());
    for (slot, edit) in reply.edits.iter_mut().zip(edits.iter()) {
        *slot = edit;
    }
    read_samples(&message[samples_at..], &mut reply.output[..samples]);
    BlockOutcome::Processed {
        flags: answer.flags,
        edit_count,
        midi_count,
    }
}

// ── Engine side: process + control channel ──────────────────────────────────

type Reply = io::Result<(u8, Vec<u8>)>;

struct ControlChannel {
    stdin: Option<ChildStdin>,
    replies: mpsc::Receiver<Reply>,
    /// A request that timed out leaves a late reply in the pipe; every later
    /// reply would be off by one, so the channel is abandoned.
    broken: bool,
}

impl ControlChannel {
    fn request(&mut self, op: Op, payload: &[u8], timeout: Duration) -> io::Result<Vec<u8>> {
        if self.broken {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "bridge control channel closed",
            ));
        }
        let result = self.exchange(op, payload, timeout);
        if matches!(&result, Err(e) if e.kind() != io::ErrorKind::Other) {
            self.broken = true;
        }
        result
    }

    fn exchange(&mut self, op: Op, payload: &[u8], timeout: Duration) -> io::Result<Vec<u8>> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "bridge stdin closed"))?;
        control::write_frame(stdin, op as u8, payload)?;
        match self.replies.recv_timeout(timeout) {
            Ok(Ok((STATUS_OK, reply))) => Ok(reply),
            Ok(Ok((_, message))) => Err(io::Error::other(
                String::from_utf8_lossy(&message).into_owned(),
            )),
            Ok(Err(error)) => Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("bridge did not answer {op:?} within {timeout:?}"),
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "bridge exited"))
            }
        }
    }
}

/// A running `plugin_host_bridge` child: its audio port and control channel.
pub struct BridgeProcess {
    audio: AudioPort,
    control: Mutex<ControlChannel>,
    child: Child,
    region: SharedRegion,
}

impl BridgeProcess {
    /// Create the region, spawn the bridge with `extra_args` (which select the
    /// plugin), and wait until it reports `READY`.
    pub fn spawn(
        bridge_exe: &Path,
        layout: BridgeLayout,
        extra_args: &[OsString],
        ready_timeout: Duration,
    ) -> io::Result<Self> {
        let region_name = unique_region_name();
        let region = SharedRegion::create(&region_name, layout.region_bytes())?;
        let audio = AudioPort::create(&region, &region_name, layout)?;

        let mut command = Command::new(bridge_exe);
        command
            .arg("--region-name")
            .arg(&region_name)
            .args(layout.to_args())
            .args(extra_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        hide_console_window(&mut command);
        let mut child = command.spawn()?;

        let stdout = child.stdout.take().expect("stdout is piped");
        let (sender, replies) = mpsc::channel::<Reply>();
        std::thread::Builder::new()
            .name("bridge-control-reader".into())
            .spawn(move || {
                let mut stdout = BufReader::new(stdout);
                loop {
                    let reply = control::read_frame(&mut stdout);
                    let failed = reply.is_err();
                    if sender.send(reply).is_err() || failed {
                        return;
                    }
                }
            })?;

        let control = Mutex::new(ControlChannel {
            stdin: child.stdin.take(),
            replies,
            broken: false,
        });
        let process = BridgeProcess {
            audio,
            control,
            child,
            region,
        };
        process.wait_ready(ready_timeout)?;
        Ok(process)
    }

    fn status(&self) -> &AtomicU32 {
        // SAFETY: the region outlives `self` and starts with the control block.
        unsafe { status_cell(self.region.as_mut_ptr()) }
    }

    fn wait_ready(&self, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.status().load(Ordering::Acquire) {
                STATUS_READY => return Ok(()),
                STATUS_ERROR => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "bridge failed to start",
                    ))
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "bridge did not become ready",
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    pub fn audio(&self) -> &AudioPort {
        &self.audio
    }

    /// Off-RT request/response on the control channel.  A timeout or a dead
    /// bridge closes the channel for good.
    pub fn request(&self, op: Op, payload: &[u8], timeout: Duration) -> io::Result<Vec<u8>> {
        self.control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .request(op, payload, timeout)
    }

    /// OS process id of the bridge.
    pub fn process_id(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for BridgeProcess {
    fn drop(&mut self) {
        self.status().store(STATUS_SHUTDOWN, Ordering::Release);
        self.audio.wake.signal();
        if let Ok(control) = self.control.get_mut() {
            control.stdin = None;
        }
        let deadline = Instant::now() + Duration::from_millis(1000);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return;
                }
            }
        }
    }
}

#[cfg(windows)]
fn hide_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_: &mut Command) {}

// ── Bridge side ─────────────────────────────────────────────────────────────

/// One block as the bridge's processor sees it.
pub struct BlockContext<'a> {
    pub frames: usize,
    /// Channels of the input samples (every input port).
    pub input_channels: usize,
    /// Channels of the output samples (every output port).
    pub output_channels: usize,
    pub has_input: bool,
    pub reset: bool,
    pub sequence: u64,
    pub steady_time: i64,
    pub transport: Option<WireTransport>,
    /// The engine's notes and raw MIDI for this block, in order.
    pub notes: Section<'a, WireNote>,
    pub params: Section<'a, WireParam>,
}

/// Fixed-capacity sink for what the plugin sends back during a block.
pub struct ReplyWriter<'a> {
    edits: &'a mut [WireParam],
    edit_count: usize,
    midi: &'a mut [WireNote],
    midi_count: usize,
}

impl<'a> ReplyWriter<'a> {
    pub fn new(edits: &'a mut [WireParam], midi: &'a mut [WireNote]) -> Self {
        ReplyWriter {
            edits,
            edit_count: 0,
            midi,
            midi_count: 0,
        }
    }

    /// Appends a parameter edit or gesture; `false` (dropped) when full.
    pub fn push_edit(&mut self, edit: WireParam) -> bool {
        let Some(slot) = self.edits.get_mut(self.edit_count) else {
            return false;
        };
        *slot = edit;
        self.edit_count += 1;
        true
    }

    /// Appends a MIDI message the plugin produced; `false` (dropped) when full.
    pub fn push_midi(&mut self, note: WireNote) -> bool {
        let Some(slot) = self.midi.get_mut(self.midi_count) else {
            return false;
        };
        *slot = note;
        self.midi_count += 1;
        true
    }

    pub fn edit_count(&self) -> usize {
        self.edit_count
    }

    pub fn midi_count(&self) -> usize {
        self.midi_count
    }
}

/// The DSP the bridge's audio loop drives.
pub trait BridgeProcessor {
    /// Channels of the reply: every active output port, read before each block
    /// because a reactivation can change the ports.
    fn output_channels(&self) -> usize;

    /// Process one block; `input`/`output` are planar.  Returns `BLOCK_*` reply
    /// flags.
    fn process(
        &mut self,
        block: &BlockContext,
        input: &[f32],
        output: &mut [f32],
        reply: &mut ReplyWriter,
    ) -> u32;

    /// Called on the audio thread when no block arrived for a while.
    fn idle(&mut self) {}
}

/// Bridge end of a device's region.  `status` may be read from any thread; the
/// rings belong to the single thread running [`serve_audio`](Self::serve_audio).
pub struct BridgePeer {
    layout: BridgeLayout,
    audio_in: ShmemRing,
    audio_out: ShmemRing,
    wake: SharedEvent,
    serving: AtomicBool,
    region: SharedRegion,
}

// SAFETY: the rings are only used inside `serve_audio`, which `serving`
// restricts to one thread; everything else is atomics or immutable.
unsafe impl Send for BridgePeer {}
unsafe impl Sync for BridgePeer {}

impl BridgePeer {
    /// Open the region the engine created and validate its control block.  On
    /// a mismatch the engine is told (`ERROR`) before the error returns.
    pub fn open(region_name: &str, layout: BridgeLayout) -> io::Result<Self> {
        let region = SharedRegion::open(region_name, layout.region_bytes())?;
        let base = region.as_mut_ptr();
        // SAFETY: the owner initialised the control block before spawning us.
        let (magic, protocol, declared) = unsafe { read_control(base) };
        if magic != CONTROL_MAGIC
            || protocol != BRIDGE_PROTOCOL_VERSION
            || declared as usize != layout.region_bytes()
        {
            unsafe { status_cell(base) }.store(STATUS_ERROR, Ordering::Release);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "control mismatch: magic={magic:#x} protocol={protocol} bytes={declared}, \
                     expected protocol={BRIDGE_PROTOCOL_VERSION} bytes={}",
                    layout.region_bytes()
                ),
            ));
        }
        let ring_bytes = layout.ring_bytes();
        // SAFETY: offsets and sizes come from the shared layout.
        let (audio_in, audio_out) = unsafe {
            (
                ShmemRing::attach(base.add(layout.audio_in_offset()), ring_bytes),
                ShmemRing::attach(base.add(layout.audio_out_offset()), ring_bytes),
            )
        };
        Ok(BridgePeer {
            layout,
            audio_in,
            audio_out,
            wake: SharedEvent::open(&wake_event_name(region_name))?,
            serving: AtomicBool::new(false),
            region,
        })
    }

    fn status(&self) -> &AtomicU32 {
        // SAFETY: see `BridgeProcess::status`.
        unsafe { status_cell(self.region.as_mut_ptr()) }
    }

    pub fn signal_ready(&self) {
        self.status().store(STATUS_READY, Ordering::Release);
    }

    pub fn signal_error(&self) {
        self.status().store(STATUS_ERROR, Ordering::Release);
    }

    pub fn shutdown_requested(&self) -> bool {
        self.status().load(Ordering::Acquire) == STATUS_SHUTDOWN
    }

    /// Stop the audio loop from inside the bridge (e.g. the engine went away
    /// without asking).
    pub fn request_shutdown(&self) {
        self.status().store(STATUS_SHUTDOWN, Ordering::Release);
        self.wake.signal();
    }

    /// The audio loop: process each block the engine sends until it asks for
    /// shutdown.  Call from one thread only.
    pub fn serve_audio(&self, processor: &mut dyn BridgeProcessor) {
        assert!(
            !self.serving.swap(true, Ordering::AcqRel),
            "BridgePeer::serve_audio runs on a single thread"
        );
        let layout = self.layout;
        let slot = layout.slot_size();
        let half_capacity = layout.event_capacity as usize / 2;
        let mut received = vec![0u8; slot];
        let mut reply = vec![0u8; slot];
        let mut input = vec![0.0f32; layout.max_samples()];
        let mut output = vec![0.0f32; layout.max_samples()];
        let mut edits = vec![WireParam::new(0, 0, 0, 0.0); half_capacity / PARAM_BYTES];
        let mut midi = vec![WireNote::new(0, 0, [0; 3]); half_capacity / NOTE_BYTES];

        while !self.shutdown_requested() {
            let Some(length) = self.audio_in.pop(&mut received) else {
                if !self.wake.wait(IDLE_WAIT_MS) {
                    processor.idle();
                }
                continue;
            };
            let message = &received[..length];
            let Some(header) = BlockHeader::from_bytes(message) else {
                continue;
            };
            let frames = (header.frames as usize).min(layout.max_frames as usize);
            let capacity = layout.channels as usize;
            let input_channels = (header.channel_count as usize).min(capacity);
            let output_channels = processor.output_channels().min(capacity);
            let input_samples = frames * input_channels;
            let output_samples = frames * output_channels;
            let transport_at = BLOCK_HEADER_BYTES.min(length);
            let notes_at = (transport_at + header.transport_bytes()).min(length);
            let params_at = (notes_at + header.note_count as usize * NOTE_BYTES).min(length);
            let samples_at = (params_at + header.param_count as usize * PARAM_BYTES).min(length);
            let has_input = header.flags & BLOCK_HAS_INPUT != 0;
            if has_input {
                read_samples(&message[samples_at..], &mut input[..input_samples]);
            } else {
                input[..input_samples].fill(0.0);
            }
            output[..output_samples].fill(0.0);

            let has_transport = header.flags & BLOCK_HAS_TRANSPORT != 0
                && notes_at - transport_at >= TRANSPORT_BYTES;
            let block = BlockContext {
                frames,
                input_channels,
                output_channels,
                has_input,
                reset: header.flags & BLOCK_RESET != 0,
                sequence: header.sequence,
                steady_time: header.steady_time,
                transport: has_transport
                    .then(|| WireTransport::read(&message[transport_at..notes_at])),
                notes: Section::new(&message[notes_at..], header.note_count as usize),
                params: Section::new(&message[params_at..], header.param_count as usize),
            };
            let mut writer = ReplyWriter::new(&mut edits, &mut midi);
            let flags = processor.process(
                &block,
                &input[..input_samples],
                &mut output[..output_samples],
                &mut writer,
            );
            let (edit_count, midi_count) = (writer.edit_count(), writer.midi_count());

            let reply_header = BlockHeader::new(BlockShape {
                frames: frames as u32,
                channel_count: output_channels as u32,
                flags,
                note_count: midi_count as u32,
                param_count: edit_count as u32,
                sequence: header.sequence,
                steady_time: header.steady_time,
            });
            reply_header.write(&mut reply);
            let mut end = write_records(&midi[..midi_count], &mut reply, BLOCK_HEADER_BYTES);
            end = write_records(&edits[..edit_count], &mut reply, end);
            let samples_end = end + output_samples * 4;
            write_samples(&output[..output_samples], &mut reply[end..samples_end]);
            self.audio_out.push(&reply[..samples_end]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shmem::OwnedRegion;

    #[test]
    fn control_block_roundtrips_on_heap_region() {
        let region = OwnedRegion::new(CONTROL_BYTES);
        let base = region.as_mut_ptr();
        unsafe { write_control_init(base, 1234) };
        assert_eq!(
            unsafe { read_control(base) },
            (CONTROL_MAGIC, BRIDGE_PROTOCOL_VERSION, 1234)
        );
        assert_eq!(
            unsafe { status_cell(base) }.load(Ordering::Acquire),
            STATUS_INIT
        );
    }

    #[test]
    fn layout_reserves_header_events_and_samples() {
        let layout = BridgeLayout::new(64, 2, 1000);
        assert_eq!(layout.slot_size(), BLOCK_HEADER_BYTES + 1000 + 64 * 2 * 4);
        assert!(layout.region_bytes() > 2 * RING_SLOTS * layout.slot_size());
    }

    #[test]
    fn the_reply_writer_refuses_overflow() {
        let mut edits = [WireParam::new(0, 0, 0, 0.0); 1];
        let mut midi = [WireNote::new(0, 0, [0; 3]); 1];
        let mut writer = ReplyWriter::new(&mut edits, &mut midi);
        assert!(writer.push_edit(WireParam::new(0, 1, 2, 3.0)));
        assert!(!writer.push_edit(WireParam::new(0, 4, 5, 6.0)));
        assert!(writer.push_midi(WireNote::new(0, 0, [1, 2, 3])));
        assert!(!writer.push_midi(WireNote::new(0, 0, [4, 5, 6])));
        assert_eq!((writer.edit_count(), writer.midi_count()), (1, 1));
    }

    #[test]
    fn control_magic_is_distinct_from_block_magic() {
        assert_ne!(CONTROL_MAGIC, crate::protocol::BLOCK_MAGIC);
    }
}

#[cfg(all(test, windows))]
mod loop_tests {
    use super::*;
    use crate::protocol::{BLOCK_PLUGIN_EDITED, MIDI, NOTE_ON, PARAM_MODULATION, PARAM_VALUE};
    use std::sync::Arc;

    /// ×2 gain; sends the engine's parameter events and notes back, adds one
    /// edit carrying the tempo of its transport, and flags every block.
    struct Doubler;

    impl BridgeProcessor for Doubler {
        fn output_channels(&self) -> usize {
            2
        }

        fn process(
            &mut self,
            block: &BlockContext,
            input: &[f32],
            output: &mut [f32],
            reply: &mut ReplyWriter,
        ) -> u32 {
            for (out, sample) in output.iter_mut().zip(input) {
                *out = sample * 2.0;
            }
            for param in block.params.iter() {
                reply.push_edit(param);
            }
            for note in block.notes.iter() {
                reply.push_midi(note);
            }
            let tempo = block.transport.map_or(0.0, |t| t.tempo);
            reply.push_edit(WireParam::new(PARAM_VALUE, 0, 99, tempo));
            BLOCK_PLUGIN_EDITED
        }
    }

    /// Owner and peer in one process, over the real OS region and event.
    fn connected(layout: BridgeLayout) -> (SharedRegion, AudioPort, Arc<BridgePeer>) {
        let name = unique_region_name();
        let region = SharedRegion::create(&name, layout.region_bytes()).unwrap();
        let port = AudioPort::create(&region, &name, layout).unwrap();
        let peer = Arc::new(BridgePeer::open(&name, layout).unwrap());
        (region, port, peer)
    }

    fn transport(tempo: f64) -> WireTransport {
        WireTransport {
            flags: 0,
            bar_number: 0,
            sample_rate: 48_000.0,
            position_samples: 0,
            position_seconds: 0.0,
            position_beats: 0.0,
            bar_start_beats: 0.0,
            tempo,
            time_signature_numerator: 4,
            time_signature_denominator: 4,
            reserved: 0,
        }
    }

    fn request<'a>(
        frames: usize,
        input: Option<&'a [f32]>,
        params: &'a [WireParam],
    ) -> BlockRequest<'a> {
        BlockRequest {
            frames,
            input,
            input_channels: 2,
            output_channels: 2,
            transport: None,
            notes: &[],
            params,
            sequence: 0,
            steady_time: 0,
            reset: false,
        }
    }

    #[test]
    fn blocks_round_trip_with_sections_and_flags() {
        let layout = BridgeLayout::new(32, 2, 1024);
        let (region, port, peer) = connected(layout);
        let server = {
            let peer = Arc::clone(&peer);
            std::thread::spawn(move || peer.serve_audio(&mut Doubler))
        };

        let mut output = vec![0.0f32; 64];
        let mut edits = vec![WireParam::new(0, 0, 0, 0.0); 8];
        let mut midi = vec![WireNote::new(0, 0, [0; 3]); 8];
        for sequence in 0..20u64 {
            let frames = 1 + (sequence as usize * 7) % 32;
            let input: Vec<f32> = (0..frames * 2)
                .map(|i| sequence as f32 + i as f32)
                .collect();
            let notes = [
                WireNote::new(0, NOTE_ON, [60, sequence as u8, 0]),
                WireNote::new(3, MIDI, [0xE0, 1, 2]),
            ];
            let params = [
                WireParam::new(PARAM_VALUE, 1, 7, sequence as f64),
                WireParam::new(PARAM_MODULATION, 2, 8, -0.5),
            ];
            let tempo = transport(100.0 + sequence as f64);
            let sent = BlockRequest {
                transport: (sequence % 2 == 0).then_some(&tempo),
                notes: &notes,
                sequence,
                ..request(frames, Some(&input), &params)
            };
            let mut reply = BlockReply {
                output: &mut output,
                edits: &mut edits,
                midi: &mut midi,
            };
            let outcome = port.process_block(&sent, &mut reply, Duration::from_secs(2));
            assert_eq!(
                outcome,
                BlockOutcome::Processed {
                    flags: crate::protocol::BLOCK_PLUGIN_EDITED,
                    edit_count: 3,
                    midi_count: 2
                }
            );
            let doubled: Vec<f32> = input.iter().map(|s| s * 2.0).collect();
            assert_eq!(
                &output[..frames * 2],
                doubled.as_slice(),
                "block {sequence}"
            );
            assert_eq!(&edits[..2], &params);
            assert_eq!(&midi[..2], &notes);
            let expected_tempo = if sequence % 2 == 0 {
                100.0 + sequence as f64
            } else {
                0.0
            };
            assert_eq!(
                edits[2].value, expected_tempo,
                "the transport arrives only when sent"
            );
        }

        peer.status().store(STATUS_SHUTDOWN, Ordering::Release);
        port.wake.signal();
        server.join().unwrap();
        drop(region);
    }

    /// Two input ports and three output ports: the block carries four channels in and
    /// six out, and each reply keeps the order of the ports.
    struct PortSplitter;

    impl BridgeProcessor for PortSplitter {
        fn output_channels(&self) -> usize {
            6
        }

        fn process(
            &mut self,
            block: &BlockContext,
            input: &[f32],
            output: &mut [f32],
            _reply: &mut ReplyWriter,
        ) -> u32 {
            assert_eq!((block.input_channels, block.output_channels), (4, 6));
            let (main, rest) = output.split_at_mut(input.len());
            for (out, sample) in main.iter_mut().zip(input) {
                *out = sample * 2.0;
            }
            rest.fill(0.25);
            0
        }
    }

    #[test]
    fn a_block_carries_the_channels_of_every_port() {
        let layout = BridgeLayout::new(16, 6, 64);
        let (region, port, peer) = connected(layout);
        let server = {
            let peer = Arc::clone(&peer);
            std::thread::spawn(move || peer.serve_audio(&mut PortSplitter))
        };
        let frames = 5;
        let input: Vec<f32> = (0..frames * 4).map(|i| i as f32).collect();
        let mut output = vec![0.0f32; 16 * 6];
        let sent = BlockRequest {
            input_channels: 4,
            output_channels: 6,
            ..request(frames, Some(&input), &[])
        };
        let mut reply = BlockReply {
            output: &mut output,
            edits: &mut [],
            midi: &mut [],
        };

        let outcome = port.process_block(&sent, &mut reply, Duration::from_secs(2));

        assert!(matches!(outcome, BlockOutcome::Processed { .. }));
        let doubled: Vec<f32> = input.iter().map(|s| s * 2.0).collect();
        assert_eq!(&output[..frames * 4], doubled.as_slice());
        assert!(output[frames * 4..frames * 6].iter().all(|&s| s == 0.25));

        peer.status().store(STATUS_SHUTDOWN, Ordering::Release);
        port.wake.signal();
        server.join().unwrap();
        drop(region);
    }

    #[test]
    fn a_block_wider_than_the_region_is_not_sent() {
        let layout = BridgeLayout::new(16, 2, 64);
        let (_region, port, _peer) = connected(layout);
        let mut output = vec![0.0f32; 16 * 4];
        let sent = BlockRequest {
            output_channels: 4,
            ..request(16, None, &[])
        };
        let mut reply = BlockReply {
            output: &mut output,
            edits: &mut [],
            midi: &mut [],
        };

        let outcome = port.process_block(&sent, &mut reply, Duration::from_millis(10));

        assert_eq!(outcome, BlockOutcome::Skipped);
    }

    #[test]
    fn watchdog_condemns_a_silent_bridge() {
        let layout = BridgeLayout::new(16, 2, 64);
        let (_region, port, _peer) = connected(layout);
        let input = vec![1.0f32; 32];
        let mut output = vec![1.0f32; 32];
        let sent = request(16, Some(&input), &[]);

        for _ in 0..DEAD_AFTER_CONSECUTIVE_TIMEOUTS {
            assert!(!port.is_dead());
            let mut reply = BlockReply {
                output: &mut output,
                edits: &mut [],
                midi: &mut [],
            };
            let outcome = port.process_block(&sent, &mut reply, Duration::ZERO);
            assert_eq!(outcome, BlockOutcome::TimedOut);
            assert!(output.iter().all(|&s| s == 0.0));
        }
        assert!(port.is_dead());
        output.fill(1.0);
        let mut reply = BlockReply {
            output: &mut output,
            edits: &mut [],
            midi: &mut [],
        };
        assert_eq!(
            port.process_block(&sent, &mut reply, Duration::from_secs(1)),
            BlockOutcome::Dead
        );
        assert!(output.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn oversized_block_is_skipped_untouched() {
        let layout = BridgeLayout::new(16, 2, 64);
        let (_region, port, _peer) = connected(layout);
        let mut output = vec![3.0f32; 64];
        let sent = request(32, None, &[]);
        let mut reply = BlockReply {
            output: &mut output,
            edits: &mut [],
            midi: &mut [],
        };
        assert_eq!(
            port.process_block(&sent, &mut reply, Duration::ZERO),
            BlockOutcome::Skipped
        );
        assert!(output.iter().all(|&s| s == 3.0));
    }

    #[test]
    fn sections_larger_than_the_event_capacity_are_skipped() {
        let layout = BridgeLayout::new(16, 2, 64);
        let (_region, port, _peer) = connected(layout);
        let mut output = vec![3.0f32; 32];
        let params = [WireParam::new(PARAM_VALUE, 0, 1, 0.0); 3];
        let sent = request(16, None, &params);
        let mut reply = BlockReply {
            output: &mut output,
            edits: &mut [],
            midi: &mut [],
        };
        assert_eq!(
            port.process_block(&sent, &mut reply, Duration::ZERO),
            BlockOutcome::Skipped
        );
    }
}

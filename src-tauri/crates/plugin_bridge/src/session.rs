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
//! [`AudioPort::process_block`] (engine audio thread) writes the block into the
//! audio-in ring, signals the wake event, and spins — bounded by a budget — for
//! the reply with the same sequence.  The bridge's audio thread
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
    read_samples, write_samples, BlockHeader, BLOCK_HAS_INPUT, BLOCK_HEADER_BYTES, BLOCK_RESET,
    BRIDGE_PROTOCOL_VERSION,
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
    /// Planar input (`channels × frames`), or `None` for a generator.
    pub input: Option<&'a [f32]>,
    /// Format-native events for this block (opaque here).
    pub events: &'a [u8],
    pub sequence: u64,
    pub steady_time: i64,
    /// Clear the plugin's DSP state before processing.
    pub reset: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOutcome {
    /// The bridge answered: `output` holds the processed block and the first
    /// `event_bytes` of `out_events` its output events.
    Processed { flags: u32, event_bytes: usize },
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
    ///
    /// `output` is planar (`channels × frames` samples used); `out_events`
    /// receives the plugin's output events.
    pub fn process_block(
        &self,
        request: &BlockRequest,
        output: &mut [f32],
        out_events: &mut [u8],
        budget: Duration,
    ) -> BlockOutcome {
        let samples = request.frames * self.layout.channels as usize;
        let fits = request.frames <= self.layout.max_frames as usize
            && output.len() >= samples
            && request.events.len() <= self.layout.event_capacity as usize
            && request.input.is_none_or(|input| input.len() >= samples);
        if !fits {
            return BlockOutcome::Skipped;
        }
        if self.dead.load(Ordering::Relaxed) {
            output[..samples].fill(0.0);
            return BlockOutcome::Dead;
        }
        if self.busy.swap(true, Ordering::Acquire) {
            return BlockOutcome::Skipped;
        }
        // SAFETY: `busy` grants exclusive use of the scratch buffers.
        let scratch = unsafe { &mut *self.scratch.get() };
        let outcome = self.round_trip(request, &mut output[..samples], out_events, budget, scratch);
        self.busy.store(false, Ordering::Release);
        self.note_outcome(outcome);
        outcome
    }

    fn round_trip(
        &self,
        request: &BlockRequest,
        output: &mut [f32],
        out_events: &mut [u8],
        budget: Duration,
        scratch: &mut PortScratch,
    ) -> BlockOutcome {
        // Replies to blocks that already timed out.
        while self.audio_out.pop(&mut scratch.recv).is_some() {}

        let mut flags = if request.input.is_some() {
            BLOCK_HAS_INPUT
        } else {
            0
        };
        if request.reset {
            flags |= BLOCK_RESET;
        }
        let header = BlockHeader::new(
            request.frames as u32,
            self.layout.channels,
            flags,
            request.events.len() as u32,
            request.sequence,
            request.steady_time,
        );
        let events_end = BLOCK_HEADER_BYTES + request.events.len();
        scratch.send[..BLOCK_HEADER_BYTES].copy_from_slice(&header.to_bytes());
        scratch.send[BLOCK_HEADER_BYTES..events_end].copy_from_slice(request.events);
        let mut len = events_end;
        if let Some(input) = request.input {
            let end = len + output.len() * 4;
            write_samples(&input[..output.len()], &mut scratch.send[len..end]);
            len = end;
        }
        if !self.audio_in.push(&scratch.send[..len]) {
            output.fill(0.0);
            return BlockOutcome::TimedOut;
        }
        self.wake.signal();

        let deadline = Instant::now() + budget;
        loop {
            if let Some(received) = self.audio_out.pop(&mut scratch.recv) {
                let message = &scratch.recv[..received];
                match BlockHeader::from_bytes(message) {
                    Some(reply) if reply.sequence == request.sequence => {
                        return read_reply(&reply, message, output, out_events);
                    }
                    _ => continue,
                }
            }
            if Instant::now() >= deadline {
                output.fill(0.0);
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
    reply: &BlockHeader,
    message: &[u8],
    output: &mut [f32],
    out_events: &mut [u8],
) -> BlockOutcome {
    let events_end = (BLOCK_HEADER_BYTES + reply.event_bytes as usize).min(message.len());
    let events = &message[BLOCK_HEADER_BYTES..events_end];
    let event_bytes = events.len().min(out_events.len());
    out_events[..event_bytes].copy_from_slice(&events[..event_bytes]);
    read_samples(&message[events_end..], output);
    BlockOutcome::Processed {
        flags: reply.flags,
        event_bytes,
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
    pub channels: usize,
    pub has_input: bool,
    pub reset: bool,
    pub sequence: u64,
    pub steady_time: i64,
    /// The engine's events for this block.
    pub events: &'a [u8],
}

/// Fixed-capacity sink for a block's output events.
pub struct EventWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> EventWriter<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        EventWriter { buf, len: 0 }
    }

    /// Appends one event; `false` (dropped) when it does not fit.
    pub fn push(&mut self, event: &[u8]) -> bool {
        let end = self.len + event.len();
        let Some(slot) = self.buf.get_mut(self.len..end) else {
            return false;
        };
        slot.copy_from_slice(event);
        self.len = end;
        true
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// The DSP the bridge's audio loop drives.
pub trait BridgeProcessor {
    /// Process one block; `input`/`output` are planar.  Returns `BLOCK_*` reply
    /// flags.
    fn process(
        &mut self,
        block: &BlockContext,
        input: &[f32],
        output: &mut [f32],
        out_events: &mut EventWriter,
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
        let event_capacity = layout.event_capacity as usize;
        let mut received = vec![0u8; slot];
        let mut reply = vec![0u8; slot];
        let mut input = vec![0.0f32; layout.max_samples()];
        let mut output = vec![0.0f32; layout.max_samples()];

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
            let samples = frames * layout.channels as usize;
            let events_end = (BLOCK_HEADER_BYTES + header.event_bytes as usize).min(length);
            let has_input = header.flags & BLOCK_HAS_INPUT != 0;
            if has_input {
                read_samples(&message[events_end..], &mut input[..samples]);
            } else {
                input[..samples].fill(0.0);
            }
            output[..samples].fill(0.0);

            let block = BlockContext {
                frames,
                channels: layout.channels as usize,
                has_input,
                reset: header.flags & BLOCK_RESET != 0,
                sequence: header.sequence,
                steady_time: header.steady_time,
                events: &message[BLOCK_HEADER_BYTES..events_end],
            };
            let out_events = &mut reply[BLOCK_HEADER_BYTES..BLOCK_HEADER_BYTES + event_capacity];
            let mut writer = EventWriter::new(out_events);
            let flags = processor.process(
                &block,
                &input[..samples],
                &mut output[..samples],
                &mut writer,
            );
            let event_bytes = writer.len();

            let reply_header = BlockHeader::new(
                frames as u32,
                layout.channels,
                flags,
                event_bytes as u32,
                header.sequence,
                header.steady_time,
            );
            reply[..BLOCK_HEADER_BYTES].copy_from_slice(&reply_header.to_bytes());
            let samples_start = BLOCK_HEADER_BYTES + event_bytes;
            let end = samples_start + samples * 4;
            write_samples(&output[..samples], &mut reply[samples_start..end]);
            self.audio_out.push(&reply[..end]);
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
    fn event_writer_refuses_overflow() {
        let mut buf = [0u8; 6];
        let mut writer = EventWriter::new(&mut buf);
        assert!(writer.push(&[1, 2, 3, 4]));
        assert!(!writer.push(&[5, 6, 7]));
        assert_eq!(writer.len(), 4);
    }

    #[test]
    fn control_magic_is_distinct_from_block_magic() {
        assert_ne!(CONTROL_MAGIC, crate::protocol::BLOCK_MAGIC);
    }
}

#[cfg(all(test, windows))]
mod loop_tests {
    use super::*;
    use std::sync::Arc;

    /// ×2 gain; echoes the engine's events back and flags every block.
    struct Doubler;

    impl BridgeProcessor for Doubler {
        fn process(
            &mut self,
            block: &BlockContext,
            input: &[f32],
            output: &mut [f32],
            out_events: &mut EventWriter,
        ) -> u32 {
            for (out, sample) in output.iter_mut().zip(input) {
                *out = sample * 2.0;
            }
            out_events.push(block.events);
            crate::protocol::BLOCK_PLUGIN_EDITED
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

    #[test]
    fn blocks_round_trip_with_events_and_flags() {
        let layout = BridgeLayout::new(32, 2, 256);
        let (region, port, peer) = connected(layout);
        let server = {
            let peer = Arc::clone(&peer);
            std::thread::spawn(move || peer.serve_audio(&mut Doubler))
        };

        let mut output = vec![0.0f32; 64];
        let mut out_events = vec![0u8; 256];
        for sequence in 0..20u64 {
            let frames = 1 + (sequence as usize * 7) % 32;
            let input: Vec<f32> = (0..frames * 2)
                .map(|i| sequence as f32 + i as f32)
                .collect();
            let events = [sequence as u8; 5];
            let request = BlockRequest {
                frames,
                input: Some(&input),
                events: &events,
                sequence,
                steady_time: 0,
                reset: false,
            };
            let outcome = port.process_block(
                &request,
                &mut output,
                &mut out_events,
                Duration::from_secs(2),
            );
            assert_eq!(
                outcome,
                BlockOutcome::Processed {
                    flags: crate::protocol::BLOCK_PLUGIN_EDITED,
                    event_bytes: 5
                }
            );
            let doubled: Vec<f32> = input.iter().map(|s| s * 2.0).collect();
            assert_eq!(
                &output[..frames * 2],
                doubled.as_slice(),
                "block {sequence}"
            );
            assert_eq!(&out_events[..5], &events);
        }

        peer.status().store(STATUS_SHUTDOWN, Ordering::Release);
        port.wake.signal();
        server.join().unwrap();
        drop(region);
    }

    #[test]
    fn watchdog_condemns_a_silent_bridge() {
        let layout = BridgeLayout::new(16, 2, 64);
        let (_region, port, _peer) = connected(layout);
        let input = vec![1.0f32; 32];
        let mut output = vec![1.0f32; 32];
        let request = BlockRequest {
            frames: 16,
            input: Some(&input),
            events: &[],
            sequence: 0,
            steady_time: 0,
            reset: false,
        };

        for _ in 0..DEAD_AFTER_CONSECUTIVE_TIMEOUTS {
            assert!(!port.is_dead());
            let outcome = port.process_block(&request, &mut output, &mut [], Duration::ZERO);
            assert_eq!(outcome, BlockOutcome::TimedOut);
            assert!(output.iter().all(|&s| s == 0.0));
        }
        assert!(port.is_dead());
        output.fill(1.0);
        assert_eq!(
            port.process_block(&request, &mut output, &mut [], Duration::from_secs(1)),
            BlockOutcome::Dead
        );
        assert!(output.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn oversized_block_is_skipped_untouched() {
        let layout = BridgeLayout::new(16, 2, 64);
        let (_region, port, _peer) = connected(layout);
        let mut output = vec![3.0f32; 64];
        let request = BlockRequest {
            frames: 32,
            input: None,
            events: &[],
            sequence: 0,
            steady_time: 0,
            reset: false,
        };
        assert_eq!(
            port.process_block(&request, &mut output, &mut [], Duration::ZERO),
            BlockOutcome::Skipped
        );
        assert!(output.iter().all(|&s| s == 3.0));
    }
}

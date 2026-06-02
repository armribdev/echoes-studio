//! Engine-side proxy node for a sandboxed, out-of-process plugin (ADR-015, 6d-iv).
//!
//! This is where the engine finally crosses the process boundary.  When a plugin
//! device is marked `sandboxed`, the compiler builds a [`BridgedNode`] instead of
//! an in-process wrapper.  The node spawns a `plugin_host_bridge` child in
//! [`prepare`](AudioNode::prepare) (off the audio thread) and, in
//! [`process`](AudioNode::process), hands each audio block to that child over the
//! shared-memory rings via [`BridgeSession::process_block`].
//!
//! ## Real-time behaviour
//!
//! `process` *does* wait for the child — out-of-process plugins are inherently
//! synchronous round-trips — but the wait is **bounded** by a spin budget.  Past
//! the budget the block degrades to silence (a stalled or crashed plugin can no
//! longer take the whole engine down — the P0-5 goal).  The round-trip itself is
//! allocation-free and lock-free (see `BridgeSession::process_block`).
//!
//! If the bridge cannot be spawned (no bridge binary, unknown DLL, spawn error),
//! the node falls back to a transparent pass-through so the graph still runs.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use plugin_bridge::session::{BridgeLayout, BridgeSession};

use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::plugin_formats::clap::host::decode_plugin_id;
use crate::plugin_system::host::PluginHost;
use crate::plugin_system::wrapper::DeviceFormat;

/// Ring-name uniqueness across nodes and re-prepares within one engine process.
static REGION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Locate the `plugin_host_bridge` executable next to the running engine binary.
pub fn locate_bridge_exe() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let exe = dir.join(format!(
        "plugin_host_bridge{}",
        std::env::consts::EXE_SUFFIX
    ));
    if exe.exists() {
        Some(exe)
    } else {
        None
    }
}

/// A plugin node hosted out-of-process.
pub struct BridgedNode {
    node_id: NodeId,
    bridge_exe: PathBuf,
    plugin_dll: PathBuf,
    /// Format-native plugin id passed to the bridge (e.g. the CLAP id).
    plugin_id: String,

    // Set at `prepare`:
    channels: usize,
    block_size: usize,
    sample_rate: u32,
    spin_budget: Duration,
    session: Option<BridgeSession>,
    planar_in: Vec<f32>,
    planar_out: Vec<f32>,
    seq: u64,
}

impl BridgedNode {
    pub fn new(
        node_id: NodeId,
        bridge_exe: PathBuf,
        plugin_dll: PathBuf,
        plugin_id: String,
    ) -> Self {
        BridgedNode {
            node_id,
            bridge_exe,
            plugin_dll,
            plugin_id,
            channels: 2,
            block_size: 0,
            sample_rate: 48_000,
            spin_budget: Duration::from_millis(5),
            session: None,
            planar_in: Vec::new(),
            planar_out: Vec::new(),
            seq: 0,
        }
    }

    /// Try to build a bridged node for `device_id`, or `None` if it cannot be
    /// sandboxed (unknown device, non-CLAP format, no DLL, or no bridge binary).
    /// The compiler falls back to the in-process path when this returns `None`.
    pub fn try_new(
        node_id: NodeId,
        device_id: &str,
        host: &PluginHost,
    ) -> Option<Box<dyn AudioNode>> {
        let manifest = host.registry().get(device_id)?;
        if manifest.format != DeviceFormat::Clap {
            return None; // VST3 sandboxing is a later step.
        }
        let dll = manifest.path.clone()?;
        let plugin_id = decode_plugin_id(device_id)?.to_string();
        let bridge_exe = locate_bridge_exe()?;
        Some(Box::new(BridgedNode::new(
            node_id, bridge_exe, dll, plugin_id,
        )))
    }

    /// Copy `inputs[0]` straight to `outputs[0]` (fallback when no bridge is up).
    fn passthrough(&self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer]) {
        if inputs.is_empty() || outputs.is_empty() {
            return; // outputs already zeroed by the scheduler
        }
        let inb = inputs[0];
        for c in 0..self.channels {
            outputs[0].channel_mut(c).copy_from_slice(inb.channel(c));
        }
    }
}

impl AudioNode for BridgedNode {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn label(&self) -> &str {
        "BridgedPlugin"
    }

    // One multichannel audio port in / out, mirroring the in-process wrapper.
    fn num_input_ports(&self) -> usize {
        1
    }
    fn num_output_ports(&self) -> usize {
        1
    }

    fn prepare(&mut self, ctx: PrepareContext) {
        self.channels = ctx.num_channels;
        self.block_size = ctx.max_block_size;
        self.sample_rate = ctx.sample_rate as u32;
        let n = ctx.num_channels * ctx.max_block_size;
        self.planar_in = vec![0.0; n];
        self.planar_out = vec![0.0; n];

        // Spin budget kept well under a block period so a slow plugin degrades to
        // silence rather than an underrun.  6e refines this with a watchdog.
        self.spin_budget = Duration::from_millis(5);

        // Spawn the bridge child once (prepare may be called again on re-compile;
        // keep the existing session if we already have one).
        if self.session.is_none() {
            let layout = BridgeLayout::new(
                ctx.max_block_size as u32,
                ctx.num_channels as u32,
                8, // ring depth
            );
            let region = format!(
                "echoes_bridge_{}_{}",
                std::process::id(),
                REGION_COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            match BridgeSession::spawn_plugin(
                &self.bridge_exe,
                &region,
                &layout,
                self.sample_rate,
                &self.plugin_dll,
                &self.plugin_id,
                Duration::from_secs(5),
            ) {
                Ok(session) => self.session = Some(session),
                Err(e) => eprintln!(
                    "[bridge] spawn failed for '{}' ({e}) — falling back to pass-through",
                    self.plugin_id
                ),
            }
        }
    }

    fn process(
        &mut self,
        inputs: &[&AudioBuffer],
        outputs: &mut [AudioBuffer],
        _transport: &TransportInfo,
    ) {
        if outputs.is_empty() {
            return;
        }
        let frames = outputs[0].num_frames;
        let nc = self.channels;

        // No live bridge, or a partial final block we did not size the rings for
        // → transparent pass-through (keeps the graph correct).
        if self.session.is_none() || frames != self.block_size {
            self.passthrough(inputs, outputs);
            return;
        }

        // Marshal inputs into the planar scratch (channel-major).
        for c in 0..nc {
            let dst = &mut self.planar_in[c * frames..c * frames + frames];
            if inputs.is_empty() {
                dst.fill(0.0);
            } else {
                dst.copy_from_slice(inputs[0].channel(c));
            }
        }

        let n = nc * frames;
        let session = self.session.as_mut().unwrap();
        // On timeout `process_block` fills `planar_out` with silence, so the copy
        // below is correct either way.
        session.process_block(
            &self.planar_in[..n],
            &mut self.planar_out[..n],
            self.seq,
            self.sample_rate,
            self.spin_budget,
        );
        self.seq += 1;

        for c in 0..nc {
            outputs[0]
                .channel_mut(c)
                .copy_from_slice(&self.planar_out[c * frames..c * frames + frames]);
        }
    }

    fn reset(&mut self) {
        // Stateless from the engine's side; the plugin keeps its own state across
        // transport stops (matching the in-process wrapper).
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> BridgedNode {
        BridgedNode::new(
            NodeId(42),
            PathBuf::from("nonexistent_bridge"),
            PathBuf::from("x.clap"),
            "clap-id".into(),
        )
    }

    #[test]
    fn declares_one_port_each() {
        let n = node();
        assert_eq!(n.node_id(), NodeId(42));
        assert_eq!(n.num_input_ports(), 1);
        assert_eq!(n.num_output_ports(), 1);
    }

    #[test]
    fn falls_back_to_passthrough_without_a_bridge() {
        // No `prepare` → no session, `block_size == 0`: `process` must take the
        // graceful pass-through path (input copied to output), never panic.
        let mut n = node();
        n.channels = 2;

        let mut input = AudioBuffer::new(2, 4);
        input.channel_mut(0).copy_from_slice(&[1.0, 2.0, 3.0, 4.0]);
        input.channel_mut(1).copy_from_slice(&[5.0, 6.0, 7.0, 8.0]);
        let inputs = [&input];
        let mut outputs = vec![AudioBuffer::new(2, 4)];

        n.process(&inputs, &mut outputs, &TransportInfo::stopped(48_000.0));

        assert_eq!(outputs[0].channel(0), &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(outputs[0].channel(1), &[5.0, 6.0, 7.0, 8.0]);
    }
}

//! Real CLAP plugin processor for the bridge (ADR-015, Phase 6d-iii-b-2).
//!
//! Implements [`BridgeProcessor`] by driving a loaded CLAP plugin's `process`
//! vtable, replacing the identity stand-in.  The audio marshalling
//! (`clap_audio_buffer` channel-pointer arrays + `clap_process`) replicates the
//! engine's proven sequence in `plugin_formats/clap/wrapper.rs`, against the same
//! [`clap_ffi`] ABI both link.
//!
//! ## What is and isn't exercised in CI
//!
//! The audio marshalling — building `clap_process` correctly and routing samples
//! through a plugin's `process` — is unit-tested against a **synthetic** CLAP
//! plugin vtable (no DLL, fully deterministic).  Loading a real `.clap` from disk
//! ([`ClapProcessor::load`]) reuses `clap_ffi`'s loader, already proven in the
//! engine; it is a manual smoke test (audit V5) since CI cannot assume a plugin
//! is installed.
//!
//! ## Scope (6d-iii-b-2)
//!
//! Audio in → process → audio out only.  Parameter / MIDI events are passed as
//! **empty** lists for now (the param/event rings are a later micro-step); the
//! 6d-iv RT proxy feeds them.

use std::ffi::{c_char, c_void};
use std::io;
use std::path::Path;
use std::sync::Arc;

use clap_ffi::{
    ClapAudioBuffer, ClapEventHeader, ClapHost, ClapLibrary, ClapOutputEvents, ClapPlugin,
    ClapPluginFactory, ClapProcess, FixedEventList, CLAP_PLUGIN_FACTORY_ID, CLAP_PROCESS_ERROR,
};
use plugin_bridge::session::{BlockContext, BridgeProcessor};

/// Output-event sink that discards everything (we do not yet route plugin →
/// host events back across the bridge).
unsafe extern "C" fn discard_out_event(
    _: *const ClapOutputEvents,
    _: *const ClapEventHeader,
) -> bool {
    false
}

/// A loaded CLAP plugin wired as a [`BridgeProcessor`].
///
/// Not `Send`/`Sync`: a CLAP plugin is single-threaded and the bridge drives it
/// from one thread only.
pub struct ClapProcessor {
    plugin: *const ClapPlugin,
    channels: usize,
    /// Max frames per block (the value passed to `activate`).
    max_frames: usize,
    in_ptrs: Vec<*mut f32>,
    out_ptrs: Vec<*mut f32>,
    in_buf: ClapAudioBuffer,
    out_buf: ClapAudioBuffer,
    /// Empty input-event list (valid, size 0).
    in_events: Box<FixedEventList>,
    out_events: ClapOutputEvents,
    /// Kept alive for the plugin's lifetime; `None` for a synthetic test plugin.
    _host: Option<Box<ClapHost>>,
    _lib: Option<Arc<ClapLibrary>>,
}

impl ClapProcessor {
    /// Load a `.clap` from disk, create the plugin, and bring it to a
    /// processing-ready state (`init` → `activate` → `start_processing`).
    pub fn load(
        dll: &Path,
        plugin_id: &str,
        sample_rate: f64,
        channels: usize,
        max_frames: usize,
    ) -> io::Result<Self> {
        let (lib, entry) =
            ClapLibrary::load(dll).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        // SAFETY: `entry` is the live clap_plugin_entry from the just-loaded DLL.
        let factory_ptr =
            unsafe { ((*entry).get_factory)(CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const c_char) };
        if factory_ptr.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "clap_entry.get_factory returned null",
            ));
        }
        let factory = factory_ptr as *const ClapPluginFactory;
        let host = clap_ffi::make_host();
        let id_c: Vec<u8> = plugin_id.bytes().chain(std::iter::once(0)).collect();

        // SAFETY: factory + host are live; id is NUL-terminated.
        let plugin = unsafe {
            ((*factory).create_plugin)(factory, host.as_ref(), id_c.as_ptr() as *const c_char)
        };
        if plugin.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("CLAP create_plugin returned null for '{plugin_id}'"),
            ));
        }

        Self::new(
            plugin,
            channels,
            sample_rate,
            max_frames,
            Some(host),
            Some(lib),
        )
    }

    /// Wrap an already-created CLAP plugin pointer, activate it, and prepare the
    /// marshalling buffers.  `host`/`lib` own the resources kept alive for the
    /// plugin's lifetime (`None` when the caller owns the plugin, e.g. tests).
    ///
    /// # Safety contract
    /// `plugin` must be a valid `*const ClapPlugin` whose vtable is fully
    /// populated and which has not yet been activated.
    fn new(
        plugin: *const ClapPlugin,
        channels: usize,
        sample_rate: f64,
        max_frames: usize,
        host: Option<Box<ClapHost>>,
        lib: Option<Arc<ClapLibrary>>,
    ) -> io::Result<Self> {
        // SAFETY: vtable calls in the CLAP-mandated order; on failure we destroy
        // the plugin before returning so no half-initialised instance leaks.
        unsafe {
            if !((*plugin).init)(plugin) {
                ((*plugin).destroy)(plugin);
                return Err(io::Error::other("CLAP plugin init failed"));
            }
            if !((*plugin).activate)(plugin, sample_rate, 1, max_frames as u32) {
                ((*plugin).destroy)(plugin);
                return Err(io::Error::other("CLAP plugin activate failed"));
            }
            ((*plugin).start_processing)(plugin);
        }

        // A null placeholder buffer; the real channel pointers are filled in each
        // `process` call.  `ClapAudioBuffer` is not `Copy`, so build two.
        let empty_buf = || ClapAudioBuffer {
            data32: std::ptr::null_mut(),
            data64: std::ptr::null_mut(),
            channel_count: channels as u32,
            latency: 0,
            constant_mask: 0,
        };

        Ok(ClapProcessor {
            plugin,
            channels,
            max_frames,
            in_ptrs: vec![std::ptr::null_mut(); channels],
            out_ptrs: vec![std::ptr::null_mut(); channels],
            in_buf: empty_buf(),
            out_buf: empty_buf(),
            in_events: FixedEventList::new(),
            out_events: ClapOutputEvents {
                ctx: std::ptr::null_mut(),
                try_push: discard_out_event,
            },
            _host: host,
            _lib: lib,
        })
    }
}

impl BridgeProcessor for ClapProcessor {
    fn process(&mut self, input: &[f32], output: &mut [f32], ctx: &BlockContext) {
        let nc = self.channels;
        let nf = ctx.block_size as usize;
        // Guard against a block larger than we activated for, or a layout that
        // does not match the buffers we were given.
        if nf == 0 || nf > self.max_frames || input.len() < nc * nf || output.len() < nc * nf {
            return;
        }

        // Planar (channel-major) layout: channel c occupies samples[c*nf .. c*nf+nf].
        for c in 0..nc {
            self.in_ptrs[c] = input[c * nf..].as_ptr() as *mut f32;
            self.out_ptrs[c] = output[c * nf..].as_mut_ptr();
        }

        self.in_buf = ClapAudioBuffer {
            data32: self.in_ptrs.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: nc as u32,
            latency: 0,
            constant_mask: 0,
        };
        self.out_buf = ClapAudioBuffer {
            data32: self.out_ptrs.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: nc as u32,
            latency: 0,
            constant_mask: 0,
        };

        let process = ClapProcess {
            steady_time: -1,
            frames_count: nf as u32,
            transport: std::ptr::null(),
            audio_inputs: &self.in_buf,
            audio_outputs: &mut self.out_buf,
            audio_inputs_count: 1,
            audio_outputs_count: 1,
            in_events: &self.in_events.input_events_vtable,
            out_events: &self.out_events,
        };

        // SAFETY: `plugin` is active and processing; the buffers referenced by
        // `process` outlive the call (they are the caller's slices + our fields).
        let status = unsafe { ((*self.plugin).process)(self.plugin, &process) };

        // On a plugin-reported error, fall back to pass-through so the block is
        // not silently dropped.
        if status == CLAP_PROCESS_ERROR {
            let n = (nc * nf).min(input.len()).min(output.len());
            output[..n].copy_from_slice(&input[..n]);
        }
    }
}

impl Drop for ClapProcessor {
    fn drop(&mut self) {
        // SAFETY: CLAP teardown order; the plugin was activated + processing.
        unsafe {
            ((*self.plugin).stop_processing)(self.plugin);
            ((*self.plugin).deactivate)(self.plugin);
            ((*self.plugin).destroy)(self.plugin);
        }
        // _host / _lib drop afterwards.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap_ffi::{ClapProcessStatus, CLAP_PROCESS_CONTINUE};

    // ── A synthetic CLAP plugin: gain ×2, no DLL, fully deterministic ──────────

    unsafe extern "C" fn ok_init(_: *const ClapPlugin) -> bool {
        true
    }
    unsafe extern "C" fn noop(_: *const ClapPlugin) {}
    unsafe extern "C" fn ok_activate(_: *const ClapPlugin, _: f64, _: u32, _: u32) -> bool {
        true
    }
    unsafe extern "C" fn ok_start(_: *const ClapPlugin) -> bool {
        true
    }
    unsafe extern "C" fn no_ext(_: *const ClapPlugin, _: *const c_char) -> *const c_void {
        std::ptr::null()
    }

    /// Reads each input channel, writes ×2 into the matching output channel.
    unsafe extern "C" fn gain2_process(
        _: *const ClapPlugin,
        process: *const ClapProcess,
    ) -> ClapProcessStatus {
        let process = &*process;
        let nf = process.frames_count as usize;
        let inb = &*process.audio_inputs;
        let outb = &*process.audio_outputs;
        let nc = inb.channel_count as usize;
        for c in 0..nc {
            let ip = *inb.data32.add(c);
            let op = *outb.data32.add(c);
            for f in 0..nf {
                *op.add(f) = *ip.add(f) * 2.0;
            }
        }
        CLAP_PROCESS_CONTINUE
    }

    fn synthetic_plugin() -> *const ClapPlugin {
        // Leaked so the pointer stays valid for the test; `destroy` is a no-op.
        let plugin = Box::new(ClapPlugin {
            desc: std::ptr::null(),
            plugin_data: std::ptr::null_mut(),
            init: ok_init,
            destroy: noop,
            activate: ok_activate,
            deactivate: noop,
            start_processing: ok_start,
            stop_processing: noop,
            reset: noop,
            process: gain2_process,
            get_extension: no_ext,
            on_main_thread: noop,
        });
        Box::into_raw(plugin) as *const ClapPlugin
    }

    #[test]
    fn drives_a_clap_process_vtable_with_correct_marshalling() {
        let channels = 2usize;
        let frames = 4usize;
        let plugin = synthetic_plugin();
        let mut proc =
            ClapProcessor::new(plugin, channels, 48_000.0, frames, None, None).expect("activate");

        // Planar: ch0 = [1,2,3,4], ch1 = [10,20,30,40].
        let input: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0];
        let mut output = vec![0.0f32; channels * frames];
        let ctx = BlockContext {
            block_size: frames as u32,
            channel_count: channels as u32,
            sample_rate: 48_000,
        };

        proc.process(&input, &mut output, &ctx);

        let expected: Vec<f32> = input.iter().map(|s| s * 2.0).collect();
        assert_eq!(
            output, expected,
            "per-channel gain via clap_process marshalling"
        );
        // `proc` drops here (no-op teardown on the synthetic); the leaked plugin
        // box is intentionally not reclaimed (test-only, process exits).
    }

    #[test]
    fn ignores_oversized_block() {
        let plugin = synthetic_plugin();
        let mut proc = ClapProcessor::new(plugin, 1, 48_000.0, 4, None, None).expect("activate");
        let input = vec![1.0f32; 4];
        let mut output = vec![0.0f32; 4];
        // block_size 8 > activated max 4 → guarded, output untouched.
        let ctx = BlockContext {
            block_size: 8,
            channel_count: 1,
            sample_rate: 48_000,
        };
        proc.process(&input, &mut output, &ctx);
        assert_eq!(output, vec![0.0; 4]);
    }
}

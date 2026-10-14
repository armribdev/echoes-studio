//! Lets an in-process CLAP plugin sleep (status + `clap.tail`).
//!
//! The rule lives in [`ProcessSleep`] (shared with the sandbox bridge); this
//! adds what only the engine knows: the audio buffers, the transport and the
//! plugin's host callbacks.  No allocation: safe on the audio thread.

use crate::audio_graph::buffer::AudioBuffer;
use crate::plugin_system::instance::PluginInstance;

use super::ffi::{is_silent, read_tail, BlockActivity, ClapProcessStatus, ProcessSleep};

#[derive(Default)]
pub struct PluginSleep {
    rule: ProcessSleep,
}

impl PluginSleep {
    pub fn new() -> Self {
        Self::default()
    }

    /// Off the audio thread, right after activation, with the instance FFI
    /// gate held: forgets stale requests and reads the plugin's `clap.tail`.
    pub fn start(&mut self, instance: &PluginInstance) {
        self.rule.wake();
        if let Some(context) = instance.clap_host_context() {
            context.take_tail_changed();
            context.take_process_request();
        }
        let tail = instance
            .clap_plugin()
            .and_then(|plugin| unsafe { read_tail(plugin) });
        self.rule.set_tail(tail);
    }

    pub fn wake(&mut self) {
        self.rule.wake();
    }

    /// Audio thread, once the block's events are in the buffer.  `true` means
    /// do not call `process` and output silence.
    pub fn skip_block(
        &mut self,
        instance: &PluginInstance,
        inputs: &[&AudioBuffer],
        frames: usize,
        has_events: bool,
        playing: bool,
    ) -> bool {
        let context = instance.clap_host_context();
        if context.is_some_and(|context| context.take_tail_changed()) {
            if let Some(plugin) = instance.clap_plugin() {
                self.rule.set_tail(unsafe { read_tail(plugin) });
            }
        }
        let input_audible =
            self.rule.wants_input_scan() && !buffers_silent(inputs.iter().copied(), frames);
        let process_requested = context.is_some_and(|context| context.take_process_request());
        self.rule.begin_block(BlockActivity {
            frames: frames as u32,
            has_events,
            input_audible,
            playing,
            process_requested,
        })
    }

    /// Audio thread, after `process` (and after the outputs are final).
    pub fn finish_block(
        &mut self,
        status: ClapProcessStatus,
        outputs: &[AudioBuffer],
        frames: usize,
    ) {
        self.rule
            .end_block(status, || buffers_silent(outputs.iter(), frames));
    }
}

pub fn clear_outputs(outputs: &mut [AudioBuffer], frames: usize) {
    for output in outputs {
        for channel in 0..output.num_channels {
            output.channel_mut(channel)[..frames].fill(0.0);
        }
    }
}

fn buffers_silent<'a>(buffers: impl Iterator<Item = &'a AudioBuffer>, frames: usize) -> bool {
    buffers.into_iter().all(|buffer| {
        (0..buffer.num_channels).all(|channel| is_silent(&buffer.channel(channel)[..frames]))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer_with(sample: f32) -> AudioBuffer {
        let mut buffer = AudioBuffer::new(2, 8);
        buffer.channel_mut(1)[3] = sample;
        buffer
    }

    #[test]
    fn a_buffer_is_judged_on_every_channel_and_only_up_to_the_block() {
        assert!(buffers_silent([buffer_with(0.0)].iter(), 8));
        assert!(!buffers_silent([buffer_with(0.5)].iter(), 8));
        assert!(buffers_silent([buffer_with(0.5)].iter(), 3));
    }

    #[test]
    fn clearing_outputs_zeroes_the_block_of_every_channel() {
        let mut outputs = [buffer_with(0.5), buffer_with(-0.5)];
        clear_outputs(&mut outputs, 8);
        assert!(buffers_silent(outputs.iter(), 8));
    }
}

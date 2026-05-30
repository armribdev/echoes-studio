use crate::audio_graph::buffer::AudioBuffer;
use crate::audio_graph::denormal::ANTI_DENORMAL;
use crate::audio_graph::node::{AudioNode, NodeId, PrepareContext, TransportInfo};
use crate::project::ReverbParams;

// Freeverb comb filter with damping
struct CombFilter {
    buf: Vec<f32>,
    pos: usize,
    filterstore: f32,
    damp1: f32,
    damp2: f32,
    feedback: f32,
}

impl CombFilter {
    fn new(size: usize) -> Self {
        CombFilter { buf: vec![0.0; size], pos: 0, filterstore: 0.0, damp1: 0.5, damp2: 0.5, feedback: 0.5 }
    }

    fn process(&mut self, input: f32) -> f32 {
        let output = self.buf[self.pos];
        // ANTI_DENORMAL keeps the LPF + comb state out of the sub-normal range
        // when input has been silent for several seconds.
        self.filterstore = output * self.damp2 + self.filterstore * self.damp1 + ANTI_DENORMAL;
        self.buf[self.pos] = input + self.filterstore * self.feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        output
    }
}

// Allpass filter (Schroeder)
struct AllpassFilter {
    buf: Vec<f32>,
    pos: usize,
}

impl AllpassFilter {
    fn new(size: usize) -> Self {
        AllpassFilter { buf: vec![0.0; size], pos: 0 }
    }

    fn process(&mut self, input: f32) -> f32 {
        let bufout = self.buf[self.pos];
        let output = -input + bufout;
        self.buf[self.pos] = input + bufout * 0.5 + ANTI_DENORMAL;
        self.pos = (self.pos + 1) % self.buf.len();
        output
    }
}

// Freeverb tunings at 44100 Hz
const COMB_TUNING_L: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING_L: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;

pub struct ReverbNode {
    id: NodeId,
    params: ReverbParams,
    combs_l: Vec<CombFilter>,
    combs_r: Vec<CombFilter>,
    allpass_l: Vec<AllpassFilter>,
    allpass_r: Vec<AllpassFilter>,
}

impl ReverbNode {
    pub fn new(id: NodeId, params: ReverbParams) -> Self {
        ReverbNode {
            id, params,
            combs_l: Vec::new(), combs_r: Vec::new(),
            allpass_l: Vec::new(), allpass_r: Vec::new(),
        }
    }
}

impl AudioNode for ReverbNode {
    fn node_id(&self) -> NodeId { self.id }
    fn label(&self) -> &str { "ReverbNode" }
    fn num_input_ports(&self) -> usize { 1 }
    fn num_output_ports(&self) -> usize { 1 }

    fn prepare(&mut self, ctx: PrepareContext) {
        let scale = ctx.sample_rate / 44100.0;
        let feedback = 0.84 * self.params.room_size;
        let damp = self.params.damping * 0.4;

        self.combs_l = COMB_TUNING_L.iter().map(|&t| {
            let mut c = CombFilter::new((t as f32 * scale) as usize);
            c.feedback = feedback;
            c.damp1 = damp;
            c.damp2 = 1.0 - damp;
            c
        }).collect();
        self.combs_r = COMB_TUNING_L.iter().map(|&t| {
            let mut c = CombFilter::new(((t + STEREO_SPREAD) as f32 * scale) as usize);
            c.feedback = feedback;
            c.damp1 = damp;
            c.damp2 = 1.0 - damp;
            c
        }).collect();
        self.allpass_l = ALLPASS_TUNING_L.iter().map(|&t| AllpassFilter::new((t as f32 * scale) as usize)).collect();
        self.allpass_r = ALLPASS_TUNING_L.iter().map(|&t| AllpassFilter::new(((t + STEREO_SPREAD) as f32 * scale) as usize)).collect();
    }

    fn process(&mut self, inputs: &[&AudioBuffer], outputs: &mut [AudioBuffer], t: &TransportInfo) {
        let input = &inputs[0];
        let output = &mut outputs[0];
        let frames = t.block_size.min(output.num_frames).min(input.num_frames);
        let channels = output.num_channels.min(input.num_channels);

        let wet = self.params.wet;
        let dry = 1.0 - wet;
        let width = self.params.width;
        let wet1 = wet * (width / 2.0 + 0.5);
        let wet2 = wet * (0.5 - width / 2.0);

        // Mono sum of input for reverb input
        for i in 0..frames {
            let in_l = if channels > 0 { input.channel(0)[i] } else { 0.0 };
            let in_r = if channels > 1 { input.channel(1)[i] } else { in_l };
            let mono = (in_l + in_r) * 0.015; // Freeverb input gain

            let mut out_l = 0.0_f32;
            let mut out_r = 0.0_f32;

            for c in &mut self.combs_l { out_l += c.process(mono); }
            for c in &mut self.combs_r { out_r += c.process(mono); }
            for a in &mut self.allpass_l { out_l = a.process(out_l); }
            for a in &mut self.allpass_r { out_r = a.process(out_r); }

            if channels > 0 { output.channel_mut(0)[i] = out_l * wet1 + out_r * wet2 + in_l * dry; }
            if channels > 1 { output.channel_mut(1)[i] = out_r * wet1 + out_l * wet2 + in_r * dry; }
        }
    }

    fn reset(&mut self) {
        for c in &mut self.combs_l { c.filterstore = 0.0; c.buf.fill(0.0); }
        for c in &mut self.combs_r { c.filterstore = 0.0; c.buf.fill(0.0); }
        for a in &mut self.allpass_l { a.buf.fill(0.0); }
        for a in &mut self.allpass_r { a.buf.fill(0.0); }
    }
}

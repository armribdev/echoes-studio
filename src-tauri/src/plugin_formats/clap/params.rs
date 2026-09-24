//! CLAP parameter plumbing shared by the effect wrapper and the instrument
//! adapter (ADR-021): parameter enumeration and per-block parameter events.

use std::collections::HashMap;
use std::ffi::CStr;

use crate::audio_graph::plugin_params::{PluginParamControl, PluginParamControls, PluginParamRange};
use crate::plugin_formats::clap::ffi::{
    ClapEventBuffer, ClapParamInfo, ClapPlugin, ClapPluginParams, CLAP_EXT_PARAMS,
    CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_MODULATABLE,
    CLAP_PARAM_IS_READONLY,
};
use crate::plugin_system::instance::PluginInstance;
use crate::plugin_system::parameter::PluginParamInfo;

fn c_text(bytes: &[u8]) -> String {
    CStr::from_bytes_until_nul(bytes)
        .map(|text| text.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn param_info(info: &ClapParamInfo) -> PluginParamInfo {
    let read_only = info.flags & CLAP_PARAM_IS_READONLY != 0;
    PluginParamInfo {
        native_id: info.id,
        name: c_text(&info.name),
        module: c_text(&info.module),
        min: info.min_value,
        max: info.max_value,
        default: info.default_value,
        automatable: info.flags & CLAP_PARAM_IS_AUTOMATABLE != 0 && !read_only,
        modulatable: info.flags & CLAP_PARAM_IS_MODULATABLE != 0 && !read_only,
        hidden: info.flags & CLAP_PARAM_IS_HIDDEN != 0,
    }
}

/// Lists the plugin's parameters through `clap.params`.
///
/// # Safety
/// `plugin` must be a live, initialised CLAP plugin, and the caller must hold
/// the instance's off-RT FFI gate (`clap.params` getters are main-thread only).
pub unsafe fn read_clap_params(plugin: *const ClapPlugin) -> Vec<PluginParamInfo> {
    let extension = ((*plugin).get_extension)(plugin, CLAP_EXT_PARAMS.as_ptr() as *const _);
    if extension.is_null() {
        return Vec::new();
    }
    let params = extension as *const ClapPluginParams;
    (0..((*params).count)(plugin))
        .filter_map(|index| {
            let mut info: ClapParamInfo = std::mem::zeroed();
            ((*params).get_info)(plugin, index, &mut info).then(|| param_info(&info))
        })
        .collect()
}

/// Parameter state of one CLAP placement and the events it sends each block.
pub struct ClapParamEvents {
    ranges: HashMap<u32, PluginParamRange>,
    controls: PluginParamControls,
    /// Parameters whose host modulation must be reset once (route removed).
    released_modulation: Vec<u32>,
    pub buffer: Box<ClapEventBuffer>,
}

impl ClapParamEvents {
    pub fn new() -> Self {
        Self {
            ranges: HashMap::new(),
            controls: PluginParamControls::default(),
            released_modulation: Vec::new(),
            buffer: ClapEventBuffer::new(),
        }
    }

    /// Off the audio thread, after `read_clap_params`.
    pub fn set_ranges(&mut self, params: &[PluginParamInfo]) {
        self.ranges = params
            .iter()
            .map(|param| {
                let range = PluginParamRange {
                    min: param.min as f32,
                    max: param.max as f32,
                    modulatable: param.modulatable,
                };
                (param.native_id, range)
            })
            .collect();
    }

    fn range(&self, native_id: u32) -> PluginParamRange {
        self.ranges.get(&native_id).copied().unwrap_or(PluginParamRange::UNKNOWN)
    }

    /// Off the audio thread, at compile time, after `set_ranges`.
    pub fn set_controls(&mut self, mut controls: Vec<PluginParamControl>, instance: &PluginInstance) {
        for control in &mut controls {
            control.range = self.range(control.native_id);
        }
        let modulated: Vec<u32> = controls
            .iter()
            .filter(|control| control.modulated && control.range.modulatable)
            .map(|control| control.native_id)
            .collect();
        self.released_modulation = instance.replace_modulated_params(&modulated);
        self.controls = PluginParamControls::new(controls);
    }

    pub fn controls_mut(&mut self) -> &mut PluginParamControls {
        &mut self.controls
    }

    /// Audio thread: refills `buffer` with this block's parameter events —
    /// queued edits, released modulation, modulation, automation.  The caller
    /// may add notes, then must call `buffer.sort()`.  No allocation.
    pub fn fill(&mut self, instance: &PluginInstance) {
        self.buffer.clear();
        let buffer = &mut self.buffer;
        let ranges = &self.ranges;

        instance.drain_params(|change| {
            let range = ranges.get(&change.native_id).copied().unwrap_or(PluginParamRange::UNKNOWN);
            buffer.push_param_value(change.sample_offset, change.native_id, range.denormalise(change.normalised) as f64);
        });

        for native_id in self.released_modulation.drain(..) {
            buffer.push_param_mod(0, native_id, 0.0);
        }

        for control in self.controls.iter() {
            if control.range.modulatable {
                for (offset, value) in control.modulation.iter() {
                    buffer.push_param_mod(offset, control.native_id, control.range.plain_offset(value) as f64);
                }
            }
            for (offset, value) in control.automation.iter() {
                let plain = control.range.denormalise(value.clamp(0.0, 1.0));
                buffer.push_param_value(offset, control.native_id, plain as f64);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_formats::clap::ffi::{
        ClapEventHeader, ClapEventParamValue, CLAP_EVENT_PARAM_MOD, CLAP_EVENT_PARAM_VALUE,
    };
    use std::sync::atomic::AtomicU32;
    use std::sync::Arc;

    fn info(native_id: u32, modulatable: bool) -> PluginParamInfo {
        PluginParamInfo {
            native_id,
            name: format!("p{native_id}"),
            module: String::new(),
            min: 100.0,
            max: 300.0,
            default: 200.0,
            automatable: true,
            modulatable,
            hidden: false,
        }
    }

    fn control(native_id: u32, modulated: bool) -> PluginParamControl {
        let mut control = PluginParamControl::new(native_id, Arc::new(AtomicU32::new(0)));
        control.modulated = modulated;
        control
    }

    fn events(params: &mut ClapParamEvents) -> Vec<(u32, u16, u32, f64)> {
        params.buffer.sort();
        (0..params.buffer.len())
            .map(|i| {
                let header = params.buffer.header(i).unwrap();
                let event = unsafe { &*(header as *const ClapEventHeader as *const ClapEventParamValue) };
                (header.time, header.r#type, event.param_id, event.value)
            })
            .collect()
    }

    #[test]
    fn modulation_becomes_a_plain_offset_and_automation_a_plain_value() {
        let instance = PluginInstance::new_internal("test", "Test");
        let mut params = ClapParamEvents::new();
        params.set_ranges(&[info(1, true), info(2, true)]);
        params.set_controls(vec![control(1, true), control(2, false)], &instance);

        params.controls_mut().modulate(1, 0.25);
        params.controls_mut().modulate_at(1, 32, -0.5);
        params.controls_mut().automate_at(2, 64, 0.5);
        params.fill(&instance);

        assert_eq!(
            events(&mut params),
            vec![
                (0, CLAP_EVENT_PARAM_MOD, 1, 50.0),
                (32, CLAP_EVENT_PARAM_MOD, 1, -100.0),
                (64, CLAP_EVENT_PARAM_VALUE, 2, 200.0),
            ]
        );
    }

    #[test]
    fn non_modulatable_parameter_ignores_modulation() {
        let instance = PluginInstance::new_internal("test", "Test");
        let mut params = ClapParamEvents::new();
        params.set_ranges(&[info(1, false)]);
        params.set_controls(vec![control(1, true)], &instance);

        params.controls_mut().modulate(1, 0.25);
        params.fill(&instance);

        assert!(events(&mut params).is_empty());
    }

    #[test]
    fn removed_modulation_is_reset_once() {
        let instance = PluginInstance::new_internal("test", "Test");
        let mut first = ClapParamEvents::new();
        first.set_ranges(&[info(1, true)]);
        first.set_controls(vec![control(1, true)], &instance);

        let mut second = ClapParamEvents::new();
        second.set_ranges(&[info(1, true)]);
        second.set_controls(Vec::new(), &instance);

        second.fill(&instance);
        assert_eq!(events(&mut second), vec![(0, CLAP_EVENT_PARAM_MOD, 1, 0.0)]);
        second.fill(&instance);
        assert!(events(&mut second).is_empty(), "the reset is sent only once");
    }

    #[test]
    fn queued_edits_are_denormalised() {
        let instance = PluginInstance::new_internal("test", "Test");
        let mut params = ClapParamEvents::new();
        params.set_ranges(&[info(1, true)]);
        assert!(instance.push_param(1, 0.5));

        params.fill(&instance);
        assert_eq!(events(&mut params), vec![(0, CLAP_EVENT_PARAM_VALUE, 1, 200.0)]);
    }
}

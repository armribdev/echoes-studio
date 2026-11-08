//! `plugin_host_bridge --scan <vst3|clap> <file>` (ADR-033, step 10): loads one plugin
//! file, lists the plugins it holds and probes the sidechain of each effect.
//!
//! The engine runs this in a process of its own with a time limit.  A file that crashes
//! or hangs in here ends only this process, and the engine blocks the file.  Nothing in
//! here is guarded: the process boundary is the guard.

use std::ffi::{c_char, CStr};
use std::path::Path;

use clap_ffi::{
    make_host, ClapAudioPortInfo, ClapHost, ClapLibrary, ClapPlugin, ClapPluginAudioPorts,
    ClapPluginDescriptor, ClapPluginFactory, CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS,
    CLAP_PLUGIN_FACTORY_ID,
};
use plugin_bridge::records::ScannedPluginRecord;
use vst3_ffi::{
    enumerate_classes, instantiate_processor, is_instrument_subcategory, IPluginFactory,
    Vst3Library,
};
use vst3_host::buses::read_aux_inputs;
use vst3_host::host_context::host_application;

/// What the plugins of a file are, once the file has been loaded.
pub fn scan_vst3(file: &Path) -> Result<Vec<ScannedPluginRecord>, String> {
    let (_library, factory) = Vst3Library::load(file)?;
    // SAFETY: `factory` is the live factory `Vst3Library::load` returned; this thread
    // is the only one using it, and it is released before the library is dropped.
    let classes = unsafe { enumerate_classes(factory) };
    let records = classes
        .into_iter()
        .map(|class| {
            let is_instrument = match &class.sub_categories {
                Some(sub_categories) => is_instrument_subcategory(sub_categories),
                None => name_says_instrument(&class.name),
            };
            let sidechain_inputs = if is_instrument {
                0
            } else {
                // SAFETY: as above.
                unsafe { vst3_aux_input_count(factory, &class.cid) }
            };
            ScannedPluginRecord {
                native_id: hex_cid(&class.cid),
                name: class.name,
                vendor: String::new(),
                is_instrument,
                sidechain_inputs,
            }
        })
        .collect();
    // SAFETY: releases the reference `Vst3Library::load` gave this scan.
    unsafe { ((*(*factory).vtbl).release)(factory) };
    Ok(records)
}

/// A throwaway component, created to read the auxiliary input buses and destroyed.
///
/// # Safety
/// `factory` must be a live `IPluginFactory`.
unsafe fn vst3_aux_input_count(factory: *mut IPluginFactory, cid: &[u8; 16]) -> u32 {
    let Ok((component, processor)) = instantiate_processor(factory, cid, host_application()) else {
        return 0;
    };
    let count = read_aux_inputs(component).len() as u32;
    ((*(*component).vtbl).terminate)(component);
    ((*(*processor).vtbl).release)(processor as *mut _);
    ((*(*component).vtbl).release)(component as *mut _);
    count
}

/// Class ids are the hexadecimal text the engine puts in a `vst3:` device id.
fn hex_cid(cid: &[u8; 16]) -> String {
    cid.iter().map(|byte| format!("{byte:02X}")).collect()
}

/// A class that does not say its sub-categories is told by its name.
fn name_says_instrument(name: &str) -> bool {
    let name = name.to_lowercase();
    name.contains("instrument") || name.contains("synth")
}

pub fn scan_clap(file: &Path) -> Result<Vec<ScannedPluginRecord>, String> {
    let (_library, entry) = ClapLibrary::load(file)?;
    // SAFETY: `entry` is the entry point of a library that stays loaded for this scan.
    let factory = unsafe {
        ((*entry).get_factory)(CLAP_PLUGIN_FACTORY_ID.as_ptr() as *const c_char)
            as *const ClapPluginFactory
    };
    if factory.is_null() {
        return Ok(Vec::new());
    }
    // One host for every probe: each creates and destroys its own plugin first.
    let host = make_host();
    // SAFETY: `factory` is non-null and belongs to the loaded library.
    let count = unsafe { ((*factory).get_plugin_count)(factory) };
    let mut records = Vec::new();
    for index in 0..count {
        // SAFETY: `index` is below the count the factory gave.
        let descriptor = unsafe { ((*factory).get_plugin_descriptor)(factory, index) };
        if descriptor.is_null() {
            continue;
        }
        // SAFETY: a non-null descriptor stays valid while the library is loaded.
        let descriptor = unsafe { &*descriptor };
        let is_instrument = descriptor_features(descriptor)
            .iter()
            .any(|feature| feature == "instrument");
        let sidechain_inputs = if is_instrument {
            0
        } else {
            clap_aux_input_count(factory, &host, descriptor.id_str())
        };
        records.push(ScannedPluginRecord {
            native_id: descriptor.id_str().to_string(),
            name: descriptor.name_str().to_string(),
            vendor: descriptor.vendor_str().to_string(),
            is_instrument,
            sidechain_inputs,
        });
    }
    Ok(records)
}

fn descriptor_features(descriptor: &ClapPluginDescriptor) -> Vec<String> {
    let mut features = Vec::new();
    if descriptor.features.is_null() {
        return features;
    }
    let mut index = 0;
    loop {
        // SAFETY: the feature list ends with a null pointer.
        let feature = unsafe { *descriptor.features.add(index) };
        if feature.is_null() {
            return features;
        }
        // SAFETY: a non-null feature is a NUL-terminated string.
        if let Ok(text) = unsafe { CStr::from_ptr(feature) }.to_str() {
            features.push(text.to_string());
        }
        index += 1;
    }
}

/// Create → init → count the auxiliary input ports → destroy.
fn clap_aux_input_count(factory: *const ClapPluginFactory, host: &ClapHost, clap_id: &str) -> u32 {
    let id: Vec<u8> = clap_id.bytes().chain(std::iter::once(0)).collect();
    // SAFETY: `factory` and `host` are live and `id` is NUL-terminated.
    unsafe {
        let plugin = ((*factory).create_plugin)(factory, host, id.as_ptr() as *const c_char);
        if plugin.is_null() {
            return 0;
        }
        let count = if ((*plugin).init)(plugin) {
            aux_input_ports(plugin)
        } else {
            0
        };
        ((*plugin).destroy)(plugin);
        count
    }
}

/// # Safety
/// `plugin` must be a live, initialised CLAP plugin.
unsafe fn aux_input_ports(plugin: *const ClapPlugin) -> u32 {
    let extension =
        ((*plugin).get_extension)(plugin, CLAP_EXT_AUDIO_PORTS.as_ptr() as *const c_char);
    if extension.is_null() {
        return 0;
    }
    let ports = extension as *const ClapPluginAudioPorts;
    let mut auxiliary = 0;
    for index in 0..((*ports).count)(plugin, true) {
        let mut info: ClapAudioPortInfo = std::mem::zeroed();
        if ((*ports).get)(plugin, index, true, &mut info)
            && info.flags & CLAP_AUDIO_PORT_IS_MAIN == 0
        {
            auxiliary += 1;
        }
    }
    auxiliary
}

/// A plugin that crashes must not leave a Windows error box waiting for a click: the
/// engine would only see the time limit pass.
#[cfg(windows)]
pub fn silence_crash_dialogs() {
    const SEM_FAILCRITICALERRORS: u32 = 0x0001;
    const SEM_NOGPFAULTERRORBOX: u32 = 0x0002;
    const SEM_NOOPENFILEERRORBOX: u32 = 0x8000;
    extern "system" {
        fn SetErrorMode(mode: u32) -> u32;
    }
    // SAFETY: plain Win32 call with no pointers.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
    }
}

#[cfg(not(windows))]
pub fn silence_crash_dialogs() {}

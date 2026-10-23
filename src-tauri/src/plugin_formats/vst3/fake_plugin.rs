//! A hand-made VST3 plugin for tests that must not depend on an installed one.
//!
//! Its component, processor and controller answer only what the host does while
//! it holds and tears down the instance; any other method aborts the test run.
//! The controller delegates `queryInterface` to a [`FakeListener`], which records
//! what the host tells it through `IInfoListener`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{c_void, CString};
use std::mem::{offset_of, size_of};
use std::ptr::{null_mut, NonNull};
use std::sync::Arc;

use super::buses::BusInfo;
use super::channel_context::{CHANNEL_COLOR_KEY, CHANNEL_NAME_KEY, CHANNEL_NAME_LENGTH_KEY};
use super::component_handler::ComponentHandler;
use super::ffi::{
    FUnknown, HostParameterChanges, IAttributeListVtbl, IAudioProcessor, IAudioProcessorVtbl,
    IBStreamVtbl, IComponent, IComponentHandlerVtbl, IComponentVtbl, IEditController,
    IEditControllerVtbl, IInfoListenerVtbl, ParameterInfo, ProcessData, ProcessSetup, TResult,
    Tuid, Vst3Controller, Vst3Library, IID_FUNKNOWN, IID_IINFO_LISTENER, IID_IUNIT_INFO, K_AUDIO,
    K_INVALID_ARGUMENT, K_NO_INTERFACE, K_OUTPUT, K_RESULT_FALSE, K_RESULT_OK,
    PARAM_IS_PROGRAM_CHANGE,
};
use super::units::{IUnitInfoVtbl, ProgramListInfo, UnitInfo};
use crate::plugin_system::instance::PluginInstance;

/// The id the fake is registered under: `vst3:` and the 32 hex digits of a class id.
pub(crate) const FAKE_PLUGIN_ID: &str = "vst3:0123456789ABCDEFFEDCBA9876543210";

/// What a controller read from the list it was given.
#[derive(Debug, PartialEq)]
pub(crate) struct Received {
    pub name: Option<String>,
    pub name_length: Option<i64>,
    pub color: Option<i64>,
}

impl Received {
    unsafe fn read(list: *mut FUnknown) -> Self {
        Self {
            name: read_text(list, CHANNEL_NAME_KEY),
            name_length: read_int(list, CHANNEL_NAME_LENGTH_KEY),
            color: read_int(list, CHANNEL_COLOR_KEY),
        }
    }
}

unsafe fn read_int(list: *mut FUnknown, name: &str) -> Option<i64> {
    let vtbl = &*((*list).vtbl as *const IAttributeListVtbl);
    let key = CString::new(name).unwrap();
    let mut value = 0;
    ((vtbl.get_int)(list, key.as_ptr(), &mut value) == K_RESULT_OK).then_some(value)
}

unsafe fn read_text(list: *mut FUnknown, name: &str) -> Option<String> {
    let vtbl = &*((*list).vtbl as *const IAttributeListVtbl);
    let key = CString::new(name).unwrap();
    let mut buffer = [0u16; 512];
    let size = size_of::<[u16; 512]>() as u32;
    let result = (vtbl.get_string)(list, key.as_ptr(), buffer.as_mut_ptr(), size);
    (result == K_RESULT_OK).then(|| {
        let length = buffer.iter().position(|&unit| unit == 0).unwrap();
        String::from_utf16_lossy(&buffer[..length])
    })
}

/// The `IInfoListener` of a controller, or a controller that has none.
#[repr(C)]
pub(crate) struct FakeListener {
    vtbl: *const IInfoListenerVtbl,
    pub references: Cell<u32>,
    listens: bool,
    pub received: RefCell<Vec<Received>>,
}

impl FakeListener {
    pub fn new(listens: bool) -> Box<Self> {
        Box::new(Self {
            vtbl: &LISTENER_VTBL,
            references: Cell::new(1),
            listens,
            received: RefCell::new(Vec::new()),
        })
    }

    pub fn as_funknown(&mut self) -> *mut FUnknown {
        self as *mut Self as *mut FUnknown
    }
}

unsafe fn listener<'a>(this: *mut FUnknown) -> &'a FakeListener {
    &*(this as *const FakeListener)
}

unsafe extern "C" fn listener_query_interface(
    this: *mut FUnknown,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    let fake = listener(this);
    if fake.listens && (*iid == IID_IINFO_LISTENER || *iid == IID_FUNKNOWN) {
        fake.references.set(fake.references.get() + 1);
        *obj = this as *mut c_void;
        return K_RESULT_OK;
    }
    *obj = null_mut();
    K_NO_INTERFACE
}

unsafe extern "C" fn listener_add_ref(this: *mut FUnknown) -> u32 {
    let fake = listener(this);
    fake.references.set(fake.references.get() + 1);
    fake.references.get()
}

unsafe extern "C" fn listener_release(this: *mut FUnknown) -> u32 {
    let fake = listener(this);
    fake.references.set(fake.references.get() - 1);
    fake.references.get()
}

unsafe extern "C" fn listener_set_channel_context_infos(
    this: *mut FUnknown,
    list: *mut FUnknown,
) -> TResult {
    listener(this)
        .received
        .borrow_mut()
        .push(Received::read(list));
    K_RESULT_OK
}

static LISTENER_VTBL: IInfoListenerVtbl = IInfoListenerVtbl {
    query_interface: listener_query_interface,
    add_ref: listener_add_ref,
    release: listener_release,
    set_channel_context_infos: listener_set_channel_context_infos,
};

/// What the processor was given by one `process` call.
#[derive(Debug, PartialEq)]
pub(crate) struct ProcessCall {
    pub num_samples: i32,
    /// `(parameter id, last value)` of each queue of the input list, in its order.
    pub changes: Vec<(u32, f64)>,
    /// How many times the component's state had been read before this call.
    pub state_reads: usize,
}

/// The `IUnitInfo` of a controller, with the parameters its program lists need.
/// Counts and failures can be forced to play a misbehaving plugin.
#[repr(C)]
pub(crate) struct FakeUnits {
    vtbl: *const IUnitInfoVtbl,
    references: Cell<u32>,
    offered: bool,
    /// `(id, parent id, name, program list id)`
    units: Vec<(i32, i32, String, i32)>,
    /// `(id, name, program names)`
    lists: Vec<(i32, String, Vec<String>)>,
    /// `(unit id, parameter id, step count)`, flagged `kIsProgramChange`.
    pub program_change_params: Vec<(i32, u32, i32)>,
    /// `(unit id, parameter id)`, without the flag.
    pub plain_params: Vec<(i32, u32)>,
    pub reported_unit_count: Option<i32>,
    pub reported_list_count: Option<i32>,
    pub reported_program_count: Option<i32>,
    pub failing_unit: Option<i32>,
    pub failing_list: Option<i32>,
    pub failing_program: Option<i32>,
    /// `(parameter id, value)` of every `setParamNormalized`, in order.
    pub selections: RefCell<Vec<(u32, f64)>>,
    /// What `getParamNormalized` answers; a selection stores its value here.
    pub values: RefCell<HashMap<u32, f64>>,
    /// Values a selection changes besides its own, as choosing a program does.
    pub selection_side_effects: Vec<(u32, f64)>,
    pub refuses_selection: bool,
    /// What the processor's `getState` writes.
    pub processor_state: Vec<u8>,
    /// What the controller was given through `setComponentState`, in order.
    pub component_states: RefCell<Vec<Vec<u8>>>,
    /// What the processor was given through `setState`, in order.
    pub component_loads: RefCell<Vec<Vec<u8>>>,
    /// What the controller was given through `setState`, in order.
    pub controller_loads: RefCell<Vec<Vec<u8>>>,
    pub refuses_component_state: bool,
    pub refuses_controller_state: bool,
    /// Every `process` call of the processor, in order.
    pub process_calls: RefCell<Vec<ProcessCall>>,
    /// How many times the processor's `getState` was called.
    pub state_reads: Cell<usize>,
    /// The host's `IComponentHandler`, as `setComponentHandler` would have given it.
    handler: Cell<*mut FUnknown>,
    /// A faulty controller: it reports to the host the value it is just given.
    pub echoes_values: bool,
    /// `(parameter id, value)` the processor writes to the output list of each
    /// block it processes, one point each, in order.
    pub outputs_to_write: Vec<(u32, f64)>,
}

impl FakeUnits {
    fn new(offered: bool) -> Box<Self> {
        Box::new(Self {
            vtbl: vtbl_of(&[
                (
                    offset_of!(IUnitInfoVtbl, add_ref),
                    units_add_ref as *const (),
                ),
                (
                    offset_of!(IUnitInfoVtbl, release),
                    units_release as *const (),
                ),
                (
                    offset_of!(IUnitInfoVtbl, get_unit_count),
                    units_get_unit_count as *const (),
                ),
                (
                    offset_of!(IUnitInfoVtbl, get_unit_info),
                    units_get_unit_info as *const (),
                ),
                (
                    offset_of!(IUnitInfoVtbl, get_program_list_count),
                    units_get_program_list_count as *const (),
                ),
                (
                    offset_of!(IUnitInfoVtbl, get_program_list_info),
                    units_get_program_list_info as *const (),
                ),
                (
                    offset_of!(IUnitInfoVtbl, get_program_name),
                    units_get_program_name as *const (),
                ),
            ]),
            references: Cell::new(0),
            offered,
            units: Vec::new(),
            lists: Vec::new(),
            program_change_params: Vec::new(),
            plain_params: Vec::new(),
            reported_unit_count: None,
            reported_list_count: None,
            reported_program_count: None,
            failing_unit: None,
            failing_list: None,
            failing_program: None,
            selections: RefCell::new(Vec::new()),
            values: RefCell::new(HashMap::new()),
            selection_side_effects: Vec::new(),
            refuses_selection: false,
            processor_state: Vec::new(),
            component_states: RefCell::new(Vec::new()),
            component_loads: RefCell::new(Vec::new()),
            controller_loads: RefCell::new(Vec::new()),
            refuses_component_state: false,
            refuses_controller_state: false,
            process_calls: RefCell::new(Vec::new()),
            state_reads: Cell::new(0),
            handler: Cell::new(null_mut()),
            echoes_values: false,
            outputs_to_write: Vec::new(),
        })
    }

    /// A controller that does not implement `IUnitInfo`.
    pub fn absent() -> Box<Self> {
        Self::new(false)
    }

    /// Only the root unit, which uses no program list.
    pub fn root_only() -> Box<Self> {
        let mut units = Self::new(true);
        units.add_unit(0, -1, "", -1);
        units
    }

    /// The root unit uses one program list.
    pub fn with_list(id: i32, name: &str, programs: &[&str]) -> Box<Self> {
        let mut units = Self::new(true);
        units.add_unit(0, -1, "", id);
        units.add_list(id, name, programs);
        units
    }

    pub fn add_unit(&mut self, id: i32, parent_id: i32, name: &str, list_id: i32) {
        self.units.push((id, parent_id, name.into(), list_id));
    }

    pub fn add_list(&mut self, id: i32, name: &str, programs: &[&str]) {
        let programs = programs.iter().map(|&program| program.into()).collect();
        self.lists.push((id, name.into(), programs));
    }

    pub fn references(&self) -> u32 {
        self.references.get()
    }

    /// `(unit id, parameter id, flags, step count)`
    fn parameters(&self) -> impl Iterator<Item = (i32, u32, i32, i32)> + '_ {
        let flagged = self
            .program_change_params
            .iter()
            .map(|&(unit, id, steps)| (unit, id, PARAM_IS_PROGRAM_CHANGE, steps));
        let plain = self.plain_params.iter().map(|&(unit, id)| (unit, id, 0, 0));
        flagged.chain(plain)
    }
}

unsafe fn units<'a>(this: *mut c_void) -> &'a FakeUnits {
    &*(this as *const FakeUnits)
}

/// Writes `text` the way a plugin fills a `String128`: cut at 128 units, with no
/// terminator when it fills the buffer.
unsafe fn write_string128(out: *mut u16, text: &str) {
    let mut written = 0;
    for (slot, unit) in text.encode_utf16().take(128).enumerate() {
        *out.add(slot) = unit;
        written = slot + 1;
    }
    if written < 128 {
        *out.add(written) = 0;
    }
}

unsafe extern "C" fn units_add_ref(this: *mut c_void) -> u32 {
    let fake = units(this);
    fake.references.set(fake.references.get() + 1);
    fake.references.get()
}

unsafe extern "C" fn units_release(this: *mut c_void) -> u32 {
    let fake = units(this);
    fake.references.set(fake.references.get() - 1);
    fake.references.get()
}

unsafe extern "C" fn units_get_unit_count(this: *mut c_void) -> i32 {
    let fake = units(this);
    fake.reported_unit_count.unwrap_or(fake.units.len() as i32)
}

unsafe extern "C" fn units_get_unit_info(
    this: *mut c_void,
    index: i32,
    info: *mut UnitInfo,
) -> TResult {
    let fake = units(this);
    let Some((id, parent_id, name, list_id)) = usize::try_from(index)
        .ok()
        .and_then(|index| fake.units.get(index))
        .filter(|_| fake.failing_unit != Some(index))
    else {
        return K_INVALID_ARGUMENT;
    };
    (*info).id = *id;
    (*info).parent_unit_id = *parent_id;
    (*info).program_list_id = *list_id;
    write_string128((*info).name.as_mut_ptr(), name);
    K_RESULT_OK
}

unsafe extern "C" fn units_get_program_list_count(this: *mut c_void) -> i32 {
    let fake = units(this);
    fake.reported_list_count.unwrap_or(fake.lists.len() as i32)
}

unsafe extern "C" fn units_get_program_list_info(
    this: *mut c_void,
    index: i32,
    info: *mut ProgramListInfo,
) -> TResult {
    let fake = units(this);
    let Some((id, name, programs)) = usize::try_from(index)
        .ok()
        .and_then(|index| fake.lists.get(index))
        .filter(|_| fake.failing_list != Some(index))
    else {
        return K_INVALID_ARGUMENT;
    };
    (*info).id = *id;
    (*info).program_count = fake.reported_program_count.unwrap_or(programs.len() as i32);
    write_string128((*info).name.as_mut_ptr(), name);
    K_RESULT_OK
}

unsafe extern "C" fn units_get_program_name(
    this: *mut c_void,
    list_id: i32,
    index: i32,
    out: *mut u16,
) -> TResult {
    let fake = units(this);
    let name = fake
        .lists
        .iter()
        .find(|(id, ..)| *id == list_id)
        .and_then(|(_, _, programs)| programs.get(usize::try_from(index).ok()?))
        .filter(|_| fake.failing_program != Some(index));
    match name {
        Some(name) => {
            write_string128(out, name);
            K_RESULT_OK
        }
        None => K_INVALID_ARGUMENT,
    }
}

unsafe extern "C" fn controller_get_param_normalized(this: *mut IEditController, id: u32) -> f64 {
    let controller = &*(this as *const FakeController);
    controller
        .units
        .as_ref()
        .and_then(|units| units.values.borrow().get(&id).copied())
        .unwrap_or(0.0)
}

/// A program-change parameter with `n` steps maps index `i` to `i / n`.
unsafe extern "C" fn controller_normalized_param_to_plain(
    this: *mut IEditController,
    id: u32,
    normalised: f64,
) -> f64 {
    let controller = &*(this as *const FakeController);
    let steps = controller.units.as_ref().and_then(|units| {
        units
            .parameters()
            .find_map(|(_, param, _, steps)| (param == id).then_some(steps))
    });
    normalised * f64::from(steps.unwrap_or(0))
}

unsafe extern "C" fn controller_plain_param_to_normalized(
    this: *mut IEditController,
    id: u32,
    plain: f64,
) -> f64 {
    let controller = &*(this as *const FakeController);
    let steps = controller.units.as_ref().and_then(|units| {
        units
            .parameters()
            .find_map(|(_, param, _, steps)| (param == id).then_some(steps))
    });
    match steps {
        Some(steps) if steps > 0 => plain / f64::from(steps),
        _ => 0.0,
    }
}

unsafe extern "C" fn controller_set_param_normalized(
    this: *mut IEditController,
    id: u32,
    value: f64,
) -> TResult {
    let controller = &*(this as *const FakeController);
    let Some(units) = controller.units.as_ref() else {
        return K_INVALID_ARGUMENT;
    };
    if units.refuses_selection {
        return K_RESULT_FALSE;
    }
    units.selections.borrow_mut().push((id, value));
    let mut values = units.values.borrow_mut();
    values.insert(id, value);
    values.extend(units.selection_side_effects.iter().copied());
    drop(values);
    let handler = units.handler.get();
    if units.echoes_values && !handler.is_null() {
        let table = &**(handler as *const *const IComponentHandlerVtbl);
        (table.begin_edit)(handler, id);
        (table.perform_edit)(handler, id, value);
        (table.end_edit)(handler, id);
    }
    K_RESULT_OK
}

unsafe extern "C" fn controller_set_component_state(
    this: *mut IEditController,
    stream: *mut c_void,
) -> TResult {
    let controller = &*(this as *const FakeController);
    let Some(units) = controller.units.as_ref() else {
        return K_INVALID_ARGUMENT;
    };
    units.component_states.borrow_mut().push(read_all(stream));
    K_RESULT_OK
}

/// Everything a plugin can read from the host's stream.
unsafe fn read_all(stream: *mut c_void) -> Vec<u8> {
    let vtbl = &**(stream as *mut *const IBStreamVtbl);
    let mut state = Vec::new();
    let mut chunk = [0u8; 64];
    loop {
        let mut read = 0;
        let result = (vtbl.read)(stream.cast(), chunk.as_mut_ptr().cast(), 64, &mut read);
        if result != K_RESULT_OK || read <= 0 {
            break;
        }
        state.extend_from_slice(&chunk[..read as usize]);
    }
    state
}

unsafe extern "C" fn controller_set_state(
    this: *mut IEditController,
    stream: *mut c_void,
) -> TResult {
    let controller = &*(this as *const FakeController);
    let Some(units) = controller.units.as_ref() else {
        return K_INVALID_ARGUMENT;
    };
    if units.refuses_controller_state {
        return K_RESULT_FALSE;
    }
    units.controller_loads.borrow_mut().push(read_all(stream));
    K_RESULT_OK
}

unsafe extern "C" fn component_set_state(this: *mut IComponent, stream: *mut c_void) -> TResult {
    let component = &*(this as *const FakeComponent);
    let Some(units) = component.units.as_ref() else {
        return K_INVALID_ARGUMENT;
    };
    if units.refuses_component_state {
        return K_RESULT_FALSE;
    }
    units.component_loads.borrow_mut().push(read_all(stream));
    K_RESULT_OK
}

pub(crate) const MONO: u64 = 1 << 19;
pub(crate) const STEREO: u64 = 0x3;
pub(crate) const SURROUND_5_1: u64 = 0x3F;

/// One audio bus of the fake: its channel count is that of its arrangement.
pub(crate) struct FakeBus {
    name: String,
    aux: bool,
    control_voltage: bool,
    arrangement: Cell<u64>,
    /// Written to every sample of every channel the host hands over; 0 = left alone.
    level: f32,
}

impl FakeBus {
    pub fn main(name: &str, arrangement: u64) -> Self {
        FakeBus {
            name: name.into(),
            aux: false,
            control_voltage: false,
            arrangement: Cell::new(arrangement),
            level: 0.0,
        }
    }

    /// The plugin renders `level` on this bus when `process` is given it.
    pub fn writing(self, level: f32) -> Self {
        FakeBus { level, ..self }
    }

    pub fn aux(name: &str, arrangement: u64) -> Self {
        FakeBus {
            aux: true,
            ..FakeBus::main(name, arrangement)
        }
    }

    pub fn control_voltage(self) -> Self {
        FakeBus {
            control_voltage: true,
            ..self
        }
    }
}

/// What the plugin saw on one input bus during `process`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SeenInput {
    pub channels: i32,
    pub silence_flags: u64,
    /// First sample of every channel; `None` for a null channel pointer.
    pub first_samples: Vec<Option<f32>>,
}

/// The audio buses of a fake plugin, and what the host did to them.
pub(crate) struct FakeBuses {
    inputs: Vec<FakeBus>,
    outputs: Vec<FakeBus>,
    /// A plugin that keeps its own layout answers `kResultFalse` to a request.
    accepts_arrangements: bool,
    undescribed_outputs: usize,
    reported_outputs: Cell<usize>,
    /// Channels of each output bus `process` was given, per call.
    process_shapes: RefCell<Vec<Vec<i32>>>,
    /// Every input bus `process` was given, per call.
    inputs_seen: RefCell<Vec<Vec<SeenInput>>>,
    arrangement_calls: RefCell<Vec<(Vec<u64>, Vec<u64>)>>,
    /// `(direction, index, state)` of every audio `activateBus`.
    activations: RefCell<Vec<(i32, i32, bool)>>,
    /// What `getLatencySamples` answers.
    latency: Cell<u32>,
}

impl FakeBuses {
    pub fn new(inputs: Vec<FakeBus>, outputs: Vec<FakeBus>) -> Box<Self> {
        Box::new(FakeBuses {
            inputs,
            outputs,
            accepts_arrangements: true,
            undescribed_outputs: 0,
            reported_outputs: Cell::new(usize::MAX),
            process_shapes: RefCell::new(Vec::new()),
            inputs_seen: RefCell::new(Vec::new()),
            arrangement_calls: RefCell::new(Vec::new()),
            activations: RefCell::new(Vec::new()),
            latency: Cell::new(0),
        })
    }

    /// The plugin now reports `samples` of latency, as after a
    /// `restartComponent(kLatencyChanged)`.
    pub fn set_latency(&self, samples: u32) {
        self.latency.set(samples);
    }

    /// The plugin counts `extra` more output buses than it will describe.
    pub fn declare_more_than_it_describes(&mut self, extra: usize) {
        self.undescribed_outputs = extra;
    }

    pub fn keeping_its_layout(mut self: Box<Self>) -> Box<Self> {
        self.accepts_arrangements = false;
        self
    }

    /// `(inputs, outputs)` of every `setBusArrangements` the host made.
    pub fn arrangement_calls(&self) -> Vec<(Vec<u64>, Vec<u64>)> {
        self.arrangement_calls.borrow().clone()
    }

    pub fn activations(&self) -> Vec<(i32, i32, bool)> {
        self.activations.borrow().clone()
    }

    /// Per `process` call, the channel count of each output bus it was given.
    pub fn process_shapes(&self) -> Vec<Vec<i32>> {
        self.process_shapes.borrow().clone()
    }

    /// Per `process` call, every input bus the plugin was given.
    pub fn inputs_seen(&self) -> Vec<Vec<SeenInput>> {
        self.inputs_seen.borrow().clone()
    }

    /// The plugin now declares only its first `count` output buses, as after a
    /// `restartComponent(kIoChanged)`.
    pub fn limit_outputs_to(&self, count: usize) {
        self.reported_outputs.set(count);
    }

    fn of_direction(&self, direction: i32) -> &[FakeBus] {
        if direction == K_OUTPUT {
            &self.outputs[..self.outputs.len().min(self.reported_outputs.get())]
        } else {
            &self.inputs
        }
    }
}

#[repr(C)]
struct FakeComponent {
    vtbl: *const IComponentVtbl,
    state: Vec<u8>,
    units: *mut FakeUnits,
    buses: *mut FakeBuses,
}

unsafe extern "C" fn component_get_state(this: *mut IComponent, stream: *mut c_void) -> TResult {
    let component = &*(this as *const FakeComponent);
    if let Some(units) = component.units.as_ref() {
        units.state_reads.set(units.state_reads.get() + 1);
    }
    let vtbl = &**(stream as *mut *const IBStreamVtbl);
    let mut written = 0;
    (vtbl.write)(
        stream.cast(),
        component.state.as_ptr() as *mut c_void,
        component.state.len() as i32,
        &mut written,
    )
}

#[repr(C)]
struct FakeController {
    vtbl: *const IEditControllerVtbl,
    listener: *mut FakeListener,
    units: *mut FakeUnits,
}

unsafe extern "C" fn controller_query_interface(
    this: *mut IEditController,
    iid: *const Tuid,
    obj: *mut *mut c_void,
) -> TResult {
    let controller = &*(this as *const FakeController);
    if let Some(units) = controller.units.as_ref().filter(|units| units.offered) {
        if *iid == IID_IUNIT_INFO {
            units.references.set(units.references.get() + 1);
            *obj = controller.units.cast();
            return K_RESULT_OK;
        }
    }
    let listener = controller.listener as *mut FUnknown;
    ((*(*listener).vtbl).query_interface)(listener, iid, obj)
}

unsafe extern "C" fn controller_get_parameter_count(this: *mut IEditController) -> i32 {
    let controller = &*(this as *const FakeController);
    controller
        .units
        .as_ref()
        .map_or(0, |units| units.parameters().count() as i32)
}

unsafe extern "C" fn controller_get_parameter_info(
    this: *mut IEditController,
    index: i32,
    info: *mut ParameterInfo,
) -> TResult {
    let controller = &*(this as *const FakeController);
    let parameter = controller
        .units
        .as_ref()
        .and_then(|units| units.parameters().nth(usize::try_from(index).ok()?));
    let Some((unit_id, id, flags, step_count)) = parameter else {
        return K_INVALID_ARGUMENT;
    };
    (*info).id = id;
    (*info).unit_id = unit_id;
    (*info).flags = flags;
    (*info).step_count = step_count;
    K_RESULT_OK
}

unsafe extern "C" fn controller_add_ref(_: *mut IEditController) -> u32 {
    1
}

unsafe extern "C" fn controller_release(_: *mut IEditController) -> u32 {
    0
}

unsafe extern "C" fn controller_set_component_handler(
    _: *mut IEditController,
    _: *mut FUnknown,
) -> TResult {
    K_RESULT_OK
}

unsafe extern "C" fn component_terminate(_: *mut IComponent) -> TResult {
    K_RESULT_OK
}

unsafe extern "C" fn component_release(_: *mut IComponent) -> u32 {
    0
}

unsafe extern "C" fn processor_release(_: *mut IAudioProcessor) -> u32 {
    0
}

/// Without [`FakeBuses`]: one audio input and one audio output, no event bus, an
/// effect.
unsafe extern "C" fn component_get_bus_count(
    this: *mut IComponent,
    media: i32,
    direction: i32,
) -> i32 {
    match (*(this as *const FakeComponent)).buses.as_ref() {
        Some(buses) if media == K_AUDIO && direction == K_OUTPUT => {
            (buses.of_direction(K_OUTPUT).len() + buses.undescribed_outputs) as i32
        }
        Some(buses) if media == K_AUDIO => buses.inputs.len() as i32,
        Some(_) => 0,
        None => i32::from(media == K_AUDIO),
    }
}

unsafe extern "C" fn component_get_bus_info(
    this: *mut IComponent,
    media: i32,
    direction: i32,
    index: i32,
    bus: *mut c_void,
) -> TResult {
    let bus = &mut *(bus as *mut BusInfo);
    let index = usize::try_from(index).unwrap_or(usize::MAX);
    let default_bus;
    let declared = match (*(this as *const FakeComponent)).buses.as_ref() {
        Some(buses) => buses.of_direction(direction).get(index),
        None => {
            let name = if direction == K_OUTPUT {
                "Stereo Out"
            } else {
                "Stereo In"
            };
            default_bus = FakeBus::main(name, STEREO);
            (index == 0).then_some(&default_bus)
        }
    };
    let Some(declared) = declared.filter(|_| media == K_AUDIO) else {
        return K_INVALID_ARGUMENT;
    };
    bus.media_type = media;
    bus.direction = direction;
    bus.channel_count = declared.arrangement.get().count_ones() as i32;
    bus.bus_type = i32::from(declared.aux);
    bus.flags = if declared.control_voltage { 2 } else { 0 };
    write_string128(bus.name.as_mut_ptr(), &declared.name);
    K_RESULT_OK
}

unsafe extern "C" fn component_activate_bus(
    this: *mut IComponent,
    media: i32,
    direction: i32,
    index: i32,
    state: u8,
) -> TResult {
    if let Some(buses) = (*(this as *const FakeComponent)).buses.as_ref() {
        if media == K_AUDIO {
            buses
                .activations
                .borrow_mut()
                .push((direction, index, state != 0));
        }
    }
    K_RESULT_OK
}

unsafe extern "C" fn component_set_active(_: *mut IComponent, _: u8) -> TResult {
    K_RESULT_OK
}

#[repr(C)]
struct FakeProcessor {
    vtbl: *const IAudioProcessorVtbl,
    units: *mut FakeUnits,
    buses: *mut FakeBuses,
}

/// With [`FakeBuses`], the request must name every bus, as the spec requires;
/// the plugin then takes it or keeps its own layout.
unsafe extern "C" fn processor_set_bus_arrangements(
    this: *mut IAudioProcessor,
    inputs: *mut u64,
    input_count: i32,
    outputs: *mut u64,
    output_count: i32,
) -> TResult {
    let Some(buses) = (*(this as *const FakeProcessor)).buses.as_ref() else {
        return K_RESULT_OK;
    };
    let read = |arrangements: *mut u64, count: i32| -> Vec<u64> {
        if arrangements.is_null() {
            return Vec::new();
        }
        std::slice::from_raw_parts(arrangements, count.max(0) as usize).to_vec()
    };
    let (inputs, outputs) = (read(inputs, input_count), read(outputs, output_count));
    buses
        .arrangement_calls
        .borrow_mut()
        .push((inputs.clone(), outputs.clone()));
    if !buses.accepts_arrangements
        || inputs.len() != buses.inputs.len()
        || outputs.len() != buses.of_direction(K_OUTPUT).len()
    {
        return K_RESULT_FALSE;
    }
    for (bus, arrangement) in buses.inputs.iter().zip(inputs) {
        bus.arrangement.set(arrangement);
    }
    for (bus, arrangement) in buses.of_direction(K_OUTPUT).iter().zip(outputs) {
        bus.arrangement.set(arrangement);
    }
    K_RESULT_OK
}

unsafe extern "C" fn processor_get_bus_arrangement(
    this: *mut IAudioProcessor,
    direction: i32,
    index: i32,
    arrangement: *mut u64,
) -> TResult {
    let declared = (*(this as *const FakeProcessor))
        .buses
        .as_ref()
        .and_then(|buses| {
            buses
                .of_direction(direction)
                .get(usize::try_from(index).ok()?)
        });
    match declared {
        Some(bus) => {
            *arrangement = bus.arrangement.get();
            K_RESULT_OK
        }
        None => K_INVALID_ARGUMENT,
    }
}

unsafe extern "C" fn processor_setup_processing(
    _: *mut IAudioProcessor,
    _: *const ProcessSetup,
) -> TResult {
    K_RESULT_OK
}

unsafe extern "C" fn processor_set_processing(_: *mut IAudioProcessor, _: u8) -> TResult {
    K_RESULT_OK
}

unsafe extern "C" fn processor_get_latency_samples(this: *mut IAudioProcessor) -> u32 {
    (*(this as *const FakeProcessor))
        .buses
        .as_ref()
        .map_or(0, |buses| buses.latency.get())
}

/// Renders each output bus the host handed over at its level.
unsafe fn write_output_buses(buses: &FakeBuses, data: &ProcessData) {
    let handed_over: &[_] = if data.outputs.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts(data.outputs, data.num_outputs.max(0) as usize)
    };
    buses
        .process_shapes
        .borrow_mut()
        .push(handed_over.iter().map(|bus| bus.num_channels).collect());
    for (bus, buffers) in buses.of_direction(K_OUTPUT).iter().zip(handed_over) {
        if bus.level == 0.0 {
            continue;
        }
        let channels =
            std::slice::from_raw_parts(buffers.channel_buffers32, buffers.num_channels as usize);
        for &channel in channels {
            std::slice::from_raw_parts_mut(channel, data.num_samples as usize).fill(bus.level);
        }
    }
}

/// Reads every input bus the host handed over, as a plugin would.
unsafe fn read_input_buses(buses: &FakeBuses, data: &ProcessData) {
    let handed_over: &[_] = if data.inputs.is_null() {
        &[]
    } else {
        std::slice::from_raw_parts(data.inputs, data.num_inputs.max(0) as usize)
    };
    let seen = handed_over
        .iter()
        .map(|bus| {
            let channels = if bus.channel_buffers32.is_null() {
                &[]
            } else {
                std::slice::from_raw_parts(bus.channel_buffers32, bus.num_channels.max(0) as usize)
            };
            SeenInput {
                channels: bus.num_channels,
                silence_flags: bus.silence_flags,
                first_samples: channels
                    .iter()
                    .map(|&channel| (!channel.is_null()).then(|| *channel))
                    .collect(),
            }
        })
        .collect();
    buses.inputs_seen.borrow_mut().push(seen);
}

/// Reads the input parameter list the way a plugin does, through its vtables.
unsafe extern "C" fn processor_process(
    this: *mut IAudioProcessor,
    data: *mut ProcessData,
) -> TResult {
    let processor = &*(this as *const FakeProcessor);
    if let Some(buses) = processor.buses.as_ref() {
        read_input_buses(buses, &*data);
        write_output_buses(buses, &*data);
    }
    let Some(units) = processor.units.as_ref() else {
        return K_RESULT_OK;
    };
    let mut changes = Vec::new();
    let list = (*data).input_param_changes as *mut HostParameterChanges;
    if !list.is_null() {
        let list_table = &*(*list).vtbl;
        for index in 0..(list_table.get_parameter_count)(list) {
            let queue = (list_table.get_parameter_data)(list, index);
            let queue_table = &*(*queue).vtbl;
            let points = (queue_table.get_point_count)(queue);
            let (mut offset, mut value) = (0, 0.0);
            if points > 0
                && (queue_table.get_point)(queue, points - 1, &mut offset, &mut value)
                    == K_RESULT_OK
            {
                changes.push(((queue_table.get_parameter_id)(queue), value));
            }
        }
    }
    let output = (*data).output_param_changes as *mut HostParameterChanges;
    if !output.is_null() {
        let table = &*(*output).vtbl;
        for &(id, value) in &units.outputs_to_write {
            let queue = (table.add_parameter_data)(output, &id, &mut 0);
            if !queue.is_null() {
                ((*(*queue).vtbl).add_point)(queue, 0, value, &mut 0);
            }
        }
    }
    units.process_calls.borrow_mut().push(ProcessCall {
        num_samples: (*data).num_samples,
        changes,
        state_reads: units.state_reads.get(),
    });
    K_RESULT_OK
}

unsafe extern "C" fn unexpected_call() {
    eprintln!("the fake VST3 plugin was called through a method it does not implement");
    std::process::abort();
}

/// A vtable of `T` (all pointers) where every method aborts but the given ones,
/// as `(offset of the field, function address)`.  Lives for the rest of the run.
fn vtbl_of<T>(methods: &[(usize, *const ())]) -> *const T {
    let mut table =
        vec![unexpected_call as *const () as usize; size_of::<T>() / size_of::<usize>()];
    for &(offset, function) in methods {
        table[offset / size_of::<usize>()] = function as usize;
    }
    Box::leak(table.into_boxed_slice()).as_ptr().cast()
}

/// A single-controller VST3 instance whose controller answers `listener`.  Nothing
/// is activated; dropping the instance tears the fakes down as for a real plugin.
pub(crate) fn fake_vst3_instance(listener: &mut FakeListener) -> Arc<PluginInstance> {
    build_instance(listener, null_mut(), null_mut())
}

/// As [`fake_vst3_instance`], with the audio buses `buses` describes.
pub(crate) fn fake_vst3_instance_with_buses(
    listener: &mut FakeListener,
    buses: &mut FakeBuses,
) -> Arc<PluginInstance> {
    build_instance(listener, null_mut(), buses)
}

/// As [`fake_vst3_instance`], with a controller that offers `units` and the
/// parameters it describes.
pub(crate) fn fake_vst3_instance_with_units(
    listener: &mut FakeListener,
    units: &mut FakeUnits,
) -> Arc<PluginInstance> {
    build_instance(listener, units, null_mut())
}

fn build_instance(
    listener: *mut FakeListener,
    units: *mut FakeUnits,
    buses: *mut FakeBuses,
) -> Arc<PluginInstance> {
    let controller = Box::leak(Box::new(FakeController {
        vtbl: vtbl_of(&[
            (
                offset_of!(IEditControllerVtbl, query_interface),
                controller_query_interface as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, add_ref),
                controller_add_ref as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, release),
                controller_release as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, get_parameter_count),
                controller_get_parameter_count as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, get_parameter_info),
                controller_get_parameter_info as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, normalized_param_to_plain),
                controller_normalized_param_to_plain as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, plain_param_to_normalized),
                controller_plain_param_to_normalized as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, get_param_normalized),
                controller_get_param_normalized as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, set_param_normalized),
                controller_set_param_normalized as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, set_component_state),
                controller_set_component_state as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, set_state),
                controller_set_state as *const (),
            ),
            (
                offset_of!(IEditControllerVtbl, set_component_handler),
                controller_set_component_handler as *const (),
            ),
        ]),
        listener,
        units,
    }));
    let state =
        unsafe { units.as_ref() }.map_or_else(Vec::new, |units| units.processor_state.clone());
    let component = Box::leak(Box::new(FakeComponent {
        state,
        units,
        buses,
        vtbl: vtbl_of::<IComponentVtbl>(&[
            (
                offset_of!(IComponentVtbl, terminate),
                component_terminate as *const (),
            ),
            (
                offset_of!(IComponentVtbl, get_bus_count),
                component_get_bus_count as *const (),
            ),
            (
                offset_of!(IComponentVtbl, get_bus_info),
                component_get_bus_info as *const (),
            ),
            (
                offset_of!(IComponentVtbl, activate_bus),
                component_activate_bus as *const (),
            ),
            (
                offset_of!(IComponentVtbl, set_active),
                component_set_active as *const (),
            ),
            (
                offset_of!(IComponentVtbl, release),
                component_release as *const (),
            ),
            (
                offset_of!(IComponentVtbl, get_state),
                component_get_state as *const (),
            ),
            (
                offset_of!(IComponentVtbl, set_state),
                component_set_state as *const (),
            ),
        ]),
    }));
    let processor = Box::leak(Box::new(FakeProcessor {
        vtbl: vtbl_of::<IAudioProcessorVtbl>(&[
            (
                offset_of!(IAudioProcessorVtbl, release),
                processor_release as *const (),
            ),
            (
                offset_of!(IAudioProcessorVtbl, set_bus_arrangements),
                processor_set_bus_arrangements as *const (),
            ),
            (
                offset_of!(IAudioProcessorVtbl, get_bus_arrangement),
                processor_get_bus_arrangement as *const (),
            ),
            (
                offset_of!(IAudioProcessorVtbl, setup_processing),
                processor_setup_processing as *const (),
            ),
            (
                offset_of!(IAudioProcessorVtbl, set_processing),
                processor_set_processing as *const (),
            ),
            (
                offset_of!(IAudioProcessorVtbl, get_latency_samples),
                processor_get_latency_samples as *const (),
            ),
            (
                offset_of!(IAudioProcessorVtbl, process),
                processor_process as *const (),
            ),
        ]),
        units,
        buses,
    }));
    let controller = NonNull::from(controller);
    let handler = ComponentHandler::boxed();
    if let Some(units) = unsafe { units.as_ref() } {
        units.handler.set(handler.as_funknown());
    }
    PluginInstance::new_vst3(
        FAKE_PLUGIN_ID,
        NonNull::from(component).cast(),
        NonNull::from(processor).cast(),
        Some(Vst3Controller::Shared(controller.cast())),
        handler,
        Vst3Library::placeholder(),
    )
}

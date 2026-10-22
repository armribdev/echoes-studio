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

use super::channel_context::{CHANNEL_COLOR_KEY, CHANNEL_NAME_KEY, CHANNEL_NAME_LENGTH_KEY};
use super::component_handler::ComponentHandler;
use super::ffi::{
    FUnknown, IAttributeListVtbl, IAudioProcessor, IAudioProcessorVtbl, IComponent, IComponentVtbl,
    IEditController, IEditControllerVtbl, IInfoListenerVtbl, ParameterInfo, TResult, Tuid,
    Vst3Controller, Vst3Library, IID_FUNKNOWN, IID_IINFO_LISTENER, IID_IUNIT_INFO,
    K_INVALID_ARGUMENT, K_NO_INTERFACE, K_RESULT_FALSE, K_RESULT_OK, PARAM_IS_PROGRAM_CHANGE,
};
use super::units::{IUnitInfoVtbl, ProgramListInfo, UnitInfo};
use crate::plugin_system::instance::PluginInstance;

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
    K_RESULT_OK
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
    build_instance(listener, null_mut())
}

/// As [`fake_vst3_instance`], with a controller that offers `units` and the
/// parameters it describes.
pub(crate) fn fake_vst3_instance_with_units(
    listener: &mut FakeListener,
    units: &mut FakeUnits,
) -> Arc<PluginInstance> {
    build_instance(listener, units)
}

fn build_instance(listener: *mut FakeListener, units: *mut FakeUnits) -> Arc<PluginInstance> {
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
                offset_of!(IEditControllerVtbl, set_component_handler),
                controller_set_component_handler as *const (),
            ),
        ]),
        listener,
        units,
    }));
    let component = Box::leak(Box::new(IComponent {
        vtbl: vtbl_of::<IComponentVtbl>(&[
            (
                offset_of!(IComponentVtbl, terminate),
                component_terminate as *const (),
            ),
            (
                offset_of!(IComponentVtbl, release),
                component_release as *const (),
            ),
        ]),
    }));
    let processor = Box::leak(Box::new(IAudioProcessor {
        vtbl: vtbl_of::<IAudioProcessorVtbl>(&[(
            offset_of!(IAudioProcessorVtbl, release),
            processor_release as *const (),
        )]),
    }));
    let controller = NonNull::from(controller);
    PluginInstance::new_vst3(
        "fake://vst3",
        NonNull::from(component),
        NonNull::from(processor),
        Some(Vst3Controller::Shared(controller.cast())),
        ComponentHandler::boxed(),
        Vst3Library::placeholder(),
    )
}

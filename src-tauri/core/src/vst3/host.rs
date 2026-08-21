//! Host-side COM objects handed to plugins.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use vst3::Steinberg::Vst::IAttributeList_::AttrID;
use vst3::Steinberg::Vst::{
    IAttributeList, IAttributeListTrait, IComponentHandler, IComponentHandlerTrait,
    IHostApplication, IHostApplicationTrait, IMessage, IMessageTrait, IParamValueQueue,
    IParameterChanges, IParameterChangesTrait, ParamID, ParamValue, String128, TChar,
};
use vst3::Steinberg::{
    int32, int64, kInvalidArgument, kNotImplemented, kResultFalse, kResultOk, tresult, uint32,
    FUnknown, TUID,
};
use vst3::{Class, ComPtr, ComWrapper, Interface};

/// Minimal IHostApplication: identifies us by name; no host-created objects.
pub struct HostApplication;

impl Class for HostApplication {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApplication {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        if name.is_null() {
            return kNotImplemented;
        }
        let out = &mut *name;
        out.fill(0);
        for (i, u) in "AudioDistillery".encode_utf16().enumerate().take(127) {
            out[i] = u;
        }
        kResultOk
    }

    /// Plugins (iZotope's hook layer among them) create IMessage objects
    /// through the host and CRASH on a null return — this must be real.
    unsafe fn createInstance(
        &self,
        cid: *mut TUID,
        _iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if obj.is_null() {
            return kInvalidArgument;
        }
        *obj = std::ptr::null_mut();
        let Some(cid) = cid.as_ref() else {
            return kInvalidArgument;
        };
        let guid: [u8; 16] = std::array::from_fn(|i| cid[i] as u8);
        if guid == IMessage::IID {
            let msg = ComWrapper::new(HostMessage::new());
            if let Some(p) = msg.to_com_ptr::<IMessage>() {
                *obj = p.into_raw() as *mut c_void;
                return kResultOk;
            }
        } else if guid == IAttributeList::IID {
            let list = ComWrapper::new(HostAttributeList::new());
            if let Some(p) = list.to_com_ptr::<IAttributeList>() {
                *obj = p.into_raw() as *mut c_void;
                return kResultOk;
            }
        }
        kNotImplemented
    }
}

enum AttrValue {
    Int(int64),
    Float(f64),
    Str(Vec<TChar>),
    Binary(Vec<u8>),
}

fn attr_key(id: AttrID) -> Option<String> {
    if id.is_null() {
        return None;
    }
    Some(unsafe { std::ffi::CStr::from_ptr(id) }.to_string_lossy().into_owned())
}

/// IAttributeList backed by a plain map. Returned string/binary pointers
/// stay valid until the attribute is overwritten (Steinberg host semantics).
pub struct HostAttributeList {
    attrs: Mutex<HashMap<String, AttrValue>>,
}

impl HostAttributeList {
    fn new() -> Self {
        Self {
            attrs: Mutex::new(HashMap::new()),
        }
    }
}

impl Class for HostAttributeList {
    type Interfaces = (IAttributeList,);
}

impl IAttributeListTrait for HostAttributeList {
    unsafe fn setInt(&self, id: AttrID, value: int64) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        self.attrs.lock().unwrap().insert(k, AttrValue::Int(value));
        kResultOk
    }
    unsafe fn getInt(&self, id: AttrID, value: *mut int64) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        match self.attrs.lock().unwrap().get(&k) {
            Some(AttrValue::Int(v)) if !value.is_null() => {
                *value = *v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setFloat(&self, id: AttrID, value: f64) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        self.attrs.lock().unwrap().insert(k, AttrValue::Float(value));
        kResultOk
    }
    unsafe fn getFloat(&self, id: AttrID, value: *mut f64) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        match self.attrs.lock().unwrap().get(&k) {
            Some(AttrValue::Float(v)) if !value.is_null() => {
                *value = *v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setString(&self, id: AttrID, string: *const TChar) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        if string.is_null() {
            return kInvalidArgument;
        }
        let mut v = Vec::new();
        let mut p = string;
        while *p != 0 {
            v.push(*p);
            p = p.add(1);
        }
        v.push(0);
        self.attrs.lock().unwrap().insert(k, AttrValue::Str(v));
        kResultOk
    }
    unsafe fn getString(&self, id: AttrID, string: *mut TChar, size_in_bytes: uint32) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        match self.attrs.lock().unwrap().get(&k) {
            Some(AttrValue::Str(v)) if !string.is_null() => {
                let cap = (size_in_bytes as usize / 2).min(v.len());
                if cap == 0 {
                    return kResultFalse;
                }
                std::ptr::copy_nonoverlapping(v.as_ptr(), string, cap);
                // Guarantee termination even when truncated.
                *string.add(cap - 1) = 0;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setBinary(&self, id: AttrID, data: *const c_void, size_in_bytes: uint32) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        if data.is_null() && size_in_bytes > 0 {
            return kInvalidArgument;
        }
        let v = std::slice::from_raw_parts(data as *const u8, size_in_bytes as usize).to_vec();
        self.attrs.lock().unwrap().insert(k, AttrValue::Binary(v));
        kResultOk
    }
    unsafe fn getBinary(
        &self,
        id: AttrID,
        data: *mut *const c_void,
        size_in_bytes: *mut uint32,
    ) -> tresult {
        let Some(k) = attr_key(id) else {
            return kInvalidArgument;
        };
        match self.attrs.lock().unwrap().get(&k) {
            Some(AttrValue::Binary(v)) if !data.is_null() && !size_in_bytes.is_null() => {
                *data = v.as_ptr() as *const c_void;
                *size_in_bytes = v.len() as uint32;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
}

/// IMessage with its own attribute list (returned un-addRef'd, Steinberg
/// host semantics — the list lives as long as the message).
pub struct HostMessage {
    id: Mutex<Vec<u8>>,
    attributes: ComWrapper<HostAttributeList>,
}

impl HostMessage {
    fn new() -> Self {
        Self {
            id: Mutex::new(vec![0]),
            attributes: ComWrapper::new(HostAttributeList::new()),
        }
    }
}

impl Class for HostMessage {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for HostMessage {
    unsafe fn getMessageID(&self) -> vst3::Steinberg::FIDString {
        self.id.lock().unwrap().as_ptr() as *const std::ffi::c_char
    }
    unsafe fn setMessageID(&self, id: vst3::Steinberg::FIDString) {
        let mut v = Vec::new();
        if !id.is_null() {
            let mut p = id;
            while *p != 0 {
                v.push(*p as u8);
                p = p.add(1);
            }
        }
        v.push(0);
        *self.id.lock().unwrap() = v;
    }
    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        self.attributes
            .as_com_ref::<IAttributeList>()
            .map(|r| r.as_ptr())
            .unwrap_or(std::ptr::null_mut())
    }
}

/// IComponentHandler: how the plugin's controller talks back to the host.
/// Edits are acknowledged (kResultOk keeps editors happy); restartComponent
/// flags accumulate atomically and the audio side drains them between
/// blocks — the VST3 mirror of the AU property-listener self-heal.
pub struct ComponentHandler {
    pub restart_flags: Arc<AtomicI32>,
}

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn performEdit(&self, _id: ParamID, _value: ParamValue) -> tresult {
        kResultOk
    }
    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        self.restart_flags.fetch_or(flags, Ordering::AcqRel);
        kResultOk
    }
}

/// Empty IParameterChanges: some plugins dereference the pointer without a
/// null check, so ProcessData always gets a real (empty) object.
pub struct NoParameterChanges;

impl Class for NoParameterChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for NoParameterChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        0
    }
    unsafe fn getParameterData(&self, _index: int32) -> *mut IParamValueQueue {
        std::ptr::null_mut()
    }
    unsafe fn addParameterData(&self, _id: *const ParamID, _index: *mut int32) -> *mut IParamValueQueue {
        std::ptr::null_mut()
    }
}

/// The process-wide host application as a borrowed FUnknown pointer.
/// The underlying object lives forever; callees addRef what they keep.
pub fn host_application_funknown() -> *mut FUnknown {
    static HOST: OnceLock<ComPtr<FUnknown>> = OnceLock::new();
    HOST.get_or_init(|| {
        ComWrapper::new(HostApplication)
            .to_com_ptr::<FUnknown>()
            .expect("HostApplication implements FUnknown")
    })
    .as_ptr()
}

//! Host-side COM objects handed to plugins.

use std::ffi::c_void;
use std::sync::OnceLock;

use vst3::Steinberg::Vst::{IHostApplication, IHostApplicationTrait, String128};
use vst3::Steinberg::{kNotImplemented, kResultOk, tresult, FUnknown, TUID};
use vst3::{Class, ComPtr, ComWrapper};

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

    unsafe fn createInstance(
        &self,
        _cid: *mut TUID,
        _iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if !obj.is_null() {
            *obj = std::ptr::null_mut();
        }
        kNotImplemented
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

//! In-memory IBStream + the two-blob state container.
//!
//! VST3 state is TWO chunks (IComponent + IEditController). Everything
//! above this module carries ONE opaque blob per plugin, so the pair is
//! packed as `"SV31" | u32le comp_len | comp | u32le ctrl_len | ctrl`.

use std::ffi::c_void;
use std::sync::Mutex;

use vst3::Steinberg::IBStream_::IStreamSeekMode_::{kIBSeekCur, kIBSeekEnd, kIBSeekSet};
use vst3::Steinberg::{
    int32, int64, kInvalidArgument, kResultFalse, kResultOk, tresult, IBStreamTrait,
};
use vst3::Class;

/// Growable in-memory stream for getState/setState exchanges.
pub struct MemoryStream {
    inner: Mutex<StreamInner>,
}

struct StreamInner {
    data: Vec<u8>,
    cursor: usize,
}

impl MemoryStream {
    /// Empty stream, for a plugin to write its state into.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(StreamInner {
                data: Vec::new(),
                cursor: 0,
            }),
        }
    }

    /// Stream positioned at 0 over existing data, for a plugin to read.
    pub fn with_data(data: &[u8]) -> Self {
        Self {
            inner: Mutex::new(StreamInner {
                data: data.to_vec(),
                cursor: 0,
            }),
        }
    }

    pub fn take_data(&self) -> Vec<u8> {
        std::mem::take(&mut self.inner.lock().unwrap().data)
    }
}

impl Class for MemoryStream {
    type Interfaces = (vst3::Steinberg::IBStream,);
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: int32, num_read: *mut int32) -> tresult {
        if buffer.is_null() || num_bytes < 0 {
            return kInvalidArgument;
        }
        let mut s = self.inner.lock().unwrap();
        let avail = s.data.len().saturating_sub(s.cursor);
        let n = (num_bytes as usize).min(avail);
        std::ptr::copy_nonoverlapping(s.data.as_ptr().add(s.cursor), buffer as *mut u8, n);
        s.cursor += n;
        if !num_read.is_null() {
            *num_read = n as int32;
        }
        kResultOk
    }

    unsafe fn write(
        &self,
        buffer: *mut c_void,
        num_bytes: int32,
        num_written: *mut int32,
    ) -> tresult {
        if buffer.is_null() || num_bytes < 0 {
            return kInvalidArgument;
        }
        let mut s = self.inner.lock().unwrap();
        let n = num_bytes as usize;
        let end = s.cursor + n;
        if end > s.data.len() {
            s.data.resize(end, 0);
        }
        let cursor = s.cursor;
        s.data[cursor..end].copy_from_slice(std::slice::from_raw_parts(buffer as *const u8, n));
        s.cursor = end;
        if !num_written.is_null() {
            *num_written = n as int32;
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let mut s = self.inner.lock().unwrap();
        let base: i64 = match mode as u32 {
            m if m == kIBSeekSet => 0,
            m if m == kIBSeekCur => s.cursor as i64,
            m if m == kIBSeekEnd => s.data.len() as i64,
            _ => return kInvalidArgument,
        };
        let target = base + pos;
        if target < 0 {
            return kResultFalse;
        }
        s.cursor = target as usize;
        if !result.is_null() {
            *result = s.cursor as int64;
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        *pos = self.inner.lock().unwrap().cursor as int64;
        kResultOk
    }
}

const MAGIC: &[u8; 4] = b"SV31";

/// Pack component + controller chunks into one opaque blob.
pub fn pack_state(component: &[u8], controller: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + component.len() + controller.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(component.len() as u32).to_le_bytes());
    out.extend_from_slice(component);
    out.extend_from_slice(&(controller.len() as u32).to_le_bytes());
    out.extend_from_slice(controller);
    out
}

/// Unpack a blob produced by `pack_state`. None on anything malformed.
pub fn unpack_state(blob: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let rest = blob.strip_prefix(MAGIC)?;
    let (len, rest) = rest.split_first_chunk::<4>()?;
    let comp_len = u32::from_le_bytes(*len) as usize;
    if rest.len() < comp_len {
        return None;
    }
    let (comp, rest) = rest.split_at(comp_len);
    let (len, rest) = rest.split_first_chunk::<4>()?;
    let ctrl_len = u32::from_le_bytes(*len) as usize;
    if rest.len() < ctrl_len {
        return None;
    }
    Some((comp.to_vec(), rest[..ctrl_len].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vst3::ComWrapper;

    #[test]
    fn container_roundtrip_and_fuzz() {
        let (c, k) = unpack_state(&pack_state(b"component", b"controller")).unwrap();
        assert_eq!(c, b"component");
        assert_eq!(k, b"controller");
        let (c, k) = unpack_state(&pack_state(b"", b"")).unwrap();
        assert!(c.is_empty() && k.is_empty());

        assert!(unpack_state(b"").is_none());
        assert!(unpack_state(b"XXXX\x01\x00\x00\x00a").is_none());
        // Truncated payloads and oversized declared lengths must fail clean.
        let good = pack_state(b"abcdef", b"ghij");
        for cut in 0..good.len() {
            let _ = unpack_state(&good[..cut]);
        }
        let mut evil = pack_state(b"abc", b"de");
        evil[4] = 0xff;
        evil[5] = 0xff;
        assert!(unpack_state(&evil).is_none());
    }

    /// Exercise the stream through its REAL COM vtable, as a plugin would.
    #[test]
    fn ibstream_roundtrip_via_vtable() {
        use vst3::Steinberg::IBStream;

        let stream = ComWrapper::new(MemoryStream::new());
        let ptr = stream.to_com_ptr::<IBStream>().unwrap();

        unsafe {
            let mut written: int32 = 0;
            let payload = b"hello vst3 state";
            assert_eq!(
                ptr.write(payload.as_ptr() as *mut c_void, payload.len() as i32, &mut written),
                kResultOk
            );
            assert_eq!(written as usize, payload.len());

            let mut pos: int64 = -1;
            assert_eq!(ptr.tell(&mut pos), kResultOk);
            assert_eq!(pos as usize, payload.len());

            assert_eq!(ptr.seek(0, kIBSeekSet as i32, std::ptr::null_mut()), kResultOk);
            let mut buf = [0u8; 64];
            let mut read: int32 = 0;
            assert_eq!(
                ptr.read(buf.as_mut_ptr() as *mut c_void, 64, &mut read),
                kResultOk
            );
            assert_eq!(&buf[..read as usize], payload);

            // seek relative + from end
            assert_eq!(ptr.seek(-4, kIBSeekEnd as i32, std::ptr::null_mut()), kResultOk);
            let mut read2: int32 = 0;
            assert_eq!(
                ptr.read(buf.as_mut_ptr() as *mut c_void, 64, &mut read2),
                kResultOk
            );
            assert_eq!(&buf[..read2 as usize], b"tate");
        }
    }
}

// Opaque sertype implementation for Cyclone DDS.
//
// Treats all data as opaque byte arrays (identity serialize/deserialize).
// Supports keyed topics via KeyDescriptors from the bridge protocol.

use std::ffi::{c_void, CString};
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::dds::bindings::*;
use crate::protocol::{KeyDescriptors, KeyField, KeyTypeHint};

/// C macro not picked up by bindgen.
pub const DDS_DOMAIN_DEFAULT: u32 = 0xFFFFFFFF;

// --- OpaqueSerdata: wraps opaque bytes ---

#[repr(C)]
pub struct OpaqueSerdata {
    pub base: ddsi_serdata,
    pub data: *mut u8,
    pub size: usize,
    /// Capacity of the Vec that was forgotten to create `data`.
    /// Required for correct Vec::from_raw_parts reconstruction.
    pub capacity: usize,
}

// --- SampleWrapper: in-memory representation passed to dds_write/dds_read ---

/// Wrapper for opaque byte data passed to/from DDS.
///
/// # Safety
///
/// When created via `from_bytes`, the wrapper borrows the slice's pointer.
/// The caller **must** ensure the source slice outlives the wrapper and any
/// DDS call that uses it. Do not store this wrapper across await points or
/// pass it to a different thread.
///
/// When populated by the DDS read path (`sd_to_sample`), the `data` pointer
/// is heap-allocated and must be freed via `free_sample_data`.
#[repr(C)]
pub struct SampleWrapper {
    pub data: *const u8,
    pub len: usize,
}

impl SampleWrapper {
    /// Create a SampleWrapper that borrows the given slice.
    ///
    /// # Safety
    ///
    /// The returned wrapper holds a raw pointer into `data`.
    /// The caller must ensure `data` is not dropped or moved while
    /// the wrapper (or a DDS call using it) is alive.
    pub unsafe fn from_bytes(data: &[u8]) -> Self {
        SampleWrapper {
            data: data.as_ptr(),
            len: data.len(),
        }
    }
}

// --- OpaqueSertype: extended sertype with key info ---

// Note: this struct uses #[repr(C)] solely to guarantee that `base` is at
// offset 0, allowing safe casting between *mut ddsi_sertype and *mut OpaqueSertype.
// The `key_fields` field is NOT C-compatible (it's a Rust Vec), but Cyclone DDS
// never accesses memory beyond the ddsi_sertype base portion.
#[repr(C)]
pub struct OpaqueSertype {
    pub base: ddsi_sertype,
    pub key_fields: Vec<KeyField>,
}

// --- Inline ref/unref (inline in C headers, not exported by bindgen) ---

unsafe fn serdata_ref(sd: *const ddsi_serdata) {
    let refc = &(*sd).refc as *const ddsrt_atomic_uint32_t as *const AtomicU32;
    (*refc).fetch_add(1, Ordering::SeqCst);
}

unsafe fn serdata_unref(sd: *mut ddsi_serdata) {
    let refc = &(*sd).refc as *const ddsrt_atomic_uint32_t as *const AtomicU32;
    if (*refc).fetch_sub(1, Ordering::SeqCst) == 1 {
        if let Some(free_fn) = (*(*sd).ops).free {
            free_fn(sd);
        }
    }
}

// --- Key comparison helpers ---

/// Read a u32 from `data` at `offset`, respecting the CDR encapsulation
/// header's endianness (bytes 0..2 of the payload).
///
/// XCDR2 encapsulation header byte[1]:
///   0x00 => big-endian, 0x01 => little-endian
fn read_u32_cdr(data: &[u8], offset: usize) -> Option<u32> {
    if offset + 4 > data.len() {
        return None;
    }
    let bytes = [data[offset], data[offset + 1], data[offset + 2], data[offset + 3]];
    // Determine endianness from encapsulation header
    let le = data.len() >= 2 && (data[1] & 0x01) != 0;
    if le {
        Some(u32::from_le_bytes(bytes))
    } else {
        Some(u32::from_be_bytes(bytes))
    }
}

/// Extract key bytes from a serdata payload for a single key field.
/// For STRING keys (CDR format): reads uint32 length + chars.
/// For fixed-size keys: reads `size` bytes at `offset`.
fn extract_key_bytes<'a>(data: &'a [u8], field: &KeyField) -> &'a [u8] {
    let offset = field.offset as usize;
    if offset >= data.len() {
        return &[];
    }
    match field.type_hint {
        KeyTypeHint::String => {
            // CDR string: uint32 length (including NUL) + chars + NUL
            let str_len = match read_u32_cdr(data, offset) {
                Some(len) => len as usize,
                None => return &[],
            };
            let end = (offset + 4 + str_len).min(data.len());
            &data[offset..end]
        }
        _ => {
            let size = field.size as usize;
            let end = (offset + size).min(data.len());
            &data[offset..end]
        }
    }
}

pub(crate) fn keys_equal(a_data: &[u8], b_data: &[u8], key_fields: &[KeyField]) -> bool {
    for field in key_fields {
        let ka = extract_key_bytes(a_data, field);
        let kb = extract_key_bytes(b_data, field);
        if ka != kb {
            return false;
        }
    }
    true
}

pub(crate) fn hash_key_bytes(data: &[u8], key_fields: &[KeyField]) -> u32 {
    let mut hash: u32 = 2166136261; // FNV-1a offset basis
    for field in key_fields {
        let kb = extract_key_bytes(data, field);
        for &b in kb {
            hash ^= b as u32;
            hash = hash.wrapping_mul(16777619);
        }
    }
    hash
}

/// Helper: forget a Vec and return (ptr, len, capacity).
fn vec_into_raw(mut v: Vec<u8>) -> (*mut u8, usize, usize) {
    let ptr = v.as_mut_ptr();
    let len = v.len();
    let cap = v.capacity();
    std::mem::forget(v);
    (ptr, len, cap)
}

// --- Serdata ops callbacks ---

unsafe extern "C" fn sd_eqkey(a: *const ddsi_serdata, b: *const ddsi_serdata) -> bool {
    let st = (*a).type_ as *const OpaqueSertype;
    if (*st).key_fields.is_empty() {
        return true; // keyless topic
    }
    let oa = &*(a as *const OpaqueSerdata);
    let ob = &*(b as *const OpaqueSerdata);
    if oa.data.is_null() || ob.data.is_null() {
        return oa.data.is_null() && ob.data.is_null();
    }
    let da = slice::from_raw_parts(oa.data, oa.size);
    let db = slice::from_raw_parts(ob.data, ob.size);
    keys_equal(da, db, &(*st).key_fields)
}

unsafe extern "C" fn sd_size(sd: *const ddsi_serdata) -> u32 {
    (*(sd as *const OpaqueSerdata)).size as u32
}

unsafe extern "C" fn sd_free(sd: *mut ddsi_serdata) {
    let osd = sd as *mut OpaqueSerdata;
    if !(*osd).data.is_null() && (*osd).capacity > 0 {
        let _ = Vec::from_raw_parts((*osd).data, (*osd).size, (*osd).capacity);
    }
    let _ = Box::from_raw(osd);
}

unsafe extern "C" fn sd_from_ser_iov(
    st: *const ddsi_sertype,
    kind: ddsi_serdata_kind,
    niov: usize,
    iov: *const ddsrt_iovec_t,
    size: usize,
) -> *mut ddsi_serdata {
    let mut buf = vec![0u8; size];
    let iovs = slice::from_raw_parts(iov, niov);
    let mut offset = 0usize;
    for io in iovs {
        let len = io.iov_len as usize;
        let to_copy = len.min(size - offset);
        if to_copy > 0 {
            ptr::copy_nonoverlapping(
                io.iov_base as *const u8,
                buf.as_mut_ptr().add(offset),
                to_copy,
            );
            offset += to_copy;
        }
    }

    let (data_ptr, data_len, data_cap) = vec_into_raw(buf);

    let osd = Box::new(OpaqueSerdata {
        base: std::mem::zeroed(),
        data: data_ptr,
        size: data_len,
        capacity: data_cap,
    });
    let osd_ptr = Box::into_raw(osd);
    ddsi_serdata_init(&mut (*osd_ptr).base, st, kind);
    osd_ptr as *mut ddsi_serdata
}

unsafe extern "C" fn sd_from_keyhash(
    _st: *const ddsi_sertype,
    _kh: *const ddsi_keyhash,
) -> *mut ddsi_serdata {
    ptr::null_mut()
}

unsafe extern "C" fn sd_from_sample(
    st: *const ddsi_sertype,
    kind: ddsi_serdata_kind,
    sample: *const c_void,
) -> *mut ddsi_serdata {
    let wrapper = sample as *const SampleWrapper;
    let src = slice::from_raw_parts((*wrapper).data, (*wrapper).len);
    let buf = src.to_vec();
    let (data_ptr, data_len, data_cap) = vec_into_raw(buf);

    let osd = Box::new(OpaqueSerdata {
        base: std::mem::zeroed(),
        data: data_ptr,
        size: data_len,
        capacity: data_cap,
    });
    let osd_ptr = Box::into_raw(osd);
    ddsi_serdata_init(&mut (*osd_ptr).base, st, kind);
    osd_ptr as *mut ddsi_serdata
}

unsafe extern "C" fn sd_to_ser(
    sd: *const ddsi_serdata,
    off: usize,
    sz: usize,
    buf: *mut c_void,
) {
    let osd = sd as *const OpaqueSerdata;
    if !(*osd).data.is_null() && off + sz <= (*osd).size {
        ptr::copy_nonoverlapping((*osd).data.add(off), buf as *mut u8, sz);
    }
}

unsafe extern "C" fn sd_to_ser_ref(
    sd: *const ddsi_serdata,
    off: usize,
    sz: usize,
    ref_: *mut ddsrt_iovec_t,
) -> *mut ddsi_serdata {
    let osd = sd as *const OpaqueSerdata;
    (*ref_).iov_base = (*osd).data.add(off) as *mut c_void;
    (*ref_).iov_len = sz as _;
    serdata_ref(sd);
    sd as *mut ddsi_serdata
}

unsafe extern "C" fn sd_to_ser_unref(sd: *mut ddsi_serdata, _ref: *const ddsrt_iovec_t) {
    serdata_unref(sd);
}

unsafe extern "C" fn sd_to_sample(
    sd: *const ddsi_serdata,
    sample: *mut c_void,
    _bufptr: *mut *mut c_void,
    _buflim: *mut c_void,
) -> bool {
    let osd = sd as *const OpaqueSerdata;
    let wrapper = sample as *mut SampleWrapper;
    (*wrapper).len = (*osd).size;
    if (*osd).size > 0 && !(*osd).data.is_null() {
        let mut buf = vec![0u8; (*osd).size];
        ptr::copy_nonoverlapping((*osd).data, buf.as_mut_ptr(), (*osd).size);
        (*wrapper).data = buf.as_ptr();
        std::mem::forget(buf);
    } else {
        (*wrapper).data = ptr::null();
    }
    true
}

unsafe extern "C" fn sd_to_untyped(sd: *const ddsi_serdata) -> *mut ddsi_serdata {
    serdata_ref(sd);
    sd as *mut ddsi_serdata
}

unsafe extern "C" fn sd_untyped_to_sample(
    _st: *const ddsi_sertype,
    sd: *const ddsi_serdata,
    sample: *mut c_void,
    bufptr: *mut *mut c_void,
    buflim: *mut c_void,
) -> bool {
    sd_to_sample(sd, sample, bufptr, buflim)
}

unsafe extern "C" fn sd_get_keyhash(
    sd: *const ddsi_serdata,
    buf: *mut ddsi_keyhash,
    _force_md5: bool,
) {
    let st = (*sd).type_ as *const OpaqueSertype;
    if (*st).key_fields.is_empty() {
        return;
    }
    let osd = sd as *const OpaqueSerdata;
    if (*osd).data.is_null() || (*osd).size == 0 {
        return;
    }
    let data = slice::from_raw_parts((*osd).data, (*osd).size);
    let hash = hash_key_bytes(data, &(*st).key_fields);
    let kh = &mut (*buf).value;
    kh[..4].copy_from_slice(&hash.to_le_bytes());
    kh[4..].fill(0);
}

// --- Sertype ops callbacks ---

unsafe extern "C" fn st_free(st: *mut ddsi_sertype) {
    let _ = Box::from_raw(st as *mut OpaqueSertype);
}

unsafe extern "C" fn st_zero_samples(
    _st: *const ddsi_sertype,
    _samples: *mut c_void,
    _count: usize,
) {
}

unsafe extern "C" fn st_realloc_samples(
    _ptrs: *mut *mut c_void,
    _st: *const ddsi_sertype,
    _old: *mut c_void,
    _oldcount: usize,
    _count: usize,
) {
}

unsafe extern "C" fn st_free_samples(
    _st: *const ddsi_sertype,
    ptrs: *mut *mut c_void,
    count: usize,
    op: dds_free_op_t,
) {
    if (op & DDS_FREE_CONTENTS_BIT) != 0 {
        for i in 0..count {
            let sample = *ptrs.add(i) as *mut SampleWrapper;
            if !(*sample).data.is_null() && (*sample).len > 0 {
                // sd_to_sample creates a Vec with exact capacity == len, then forgets it.
                let _ = Vec::from_raw_parts((*sample).data as *mut u8, (*sample).len, (*sample).len);
                (*sample).data = ptr::null();
            }
        }
    }
    if (op & DDS_FREE_ALL_BIT) != 0 {
        for i in 0..count {
            let _ = Box::from_raw(*ptrs.add(i) as *mut SampleWrapper);
        }
    }
}

unsafe extern "C" fn st_equal(a: *const ddsi_sertype, b: *const ddsi_sertype) -> bool {
    let name_a = std::ffi::CStr::from_ptr((*a).type_name);
    let name_b = std::ffi::CStr::from_ptr((*b).type_name);
    name_a == name_b
}

unsafe extern "C" fn st_hash(st: *const ddsi_sertype) -> u32 {
    let name = std::ffi::CStr::from_ptr((*st).type_name);
    let mut h: u32 = 0;
    for &b in name.to_bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as u32);
    }
    h
}

// --- Static ops tables ---

static SERDATA_OPS: ddsi_serdata_ops = ddsi_serdata_ops {
    eqkey: Some(sd_eqkey),
    get_size: Some(sd_size),
    from_ser: None,
    from_ser_iov: Some(sd_from_ser_iov),
    from_keyhash: Some(sd_from_keyhash),
    from_sample: Some(sd_from_sample),
    to_ser: Some(sd_to_ser),
    to_ser_ref: Some(sd_to_ser_ref),
    to_ser_unref: Some(sd_to_ser_unref),
    to_sample: Some(sd_to_sample),
    to_untyped: Some(sd_to_untyped),
    untyped_to_sample: Some(sd_untyped_to_sample),
    free: Some(sd_free),
    print: None,
    get_keyhash: Some(sd_get_keyhash),
    from_loaned_sample: None,
    from_psmx: None,
};

// Safety: the `arg` field is null and never written to.
// The ops tables are read-only after initialization.
unsafe impl Sync for SertypeOpsWrapper {}
struct SertypeOpsWrapper(ddsi_sertype_ops);

static SERTYPE_OPS_WRAPPER: SertypeOpsWrapper = SertypeOpsWrapper(ddsi_sertype_ops {
    version: None,
    arg: ptr::null_mut(),
    free: Some(st_free),
    zero_samples: Some(st_zero_samples),
    realloc_samples: Some(st_realloc_samples),
    free_samples: Some(st_free_samples),
    equal: Some(st_equal),
    hash: Some(st_hash),
    type_id: None,
    type_map: None,
    type_info: None,
    derive_sertype: None,
    get_serialized_size: None,
    serialize_into: None,
});

/// Create an opaque sertype for Cyclone DDS with optional key descriptors.
///
/// The returned pointer is owned by Cyclone DDS after being passed to
/// `dds_create_topic_sertype`. Do not free it manually after that.
pub unsafe fn create_opaque_sertype(
    type_name: &str,
    key_descriptors: &KeyDescriptors,
) -> *mut ddsi_sertype {
    let c_name = CString::new(type_name).expect("type_name contains NUL");
    let has_keys = !key_descriptors.keys.is_empty();

    let ost = Box::new(OpaqueSertype {
        base: std::mem::zeroed(),
        key_fields: key_descriptors.keys.clone(),
    });
    let ost_ptr = Box::into_raw(ost);

    ddsi_sertype_init(
        &mut (*ost_ptr).base,
        c_name.as_ptr(),
        &SERTYPE_OPS_WRAPPER.0,
        &SERDATA_OPS,
        !has_keys, // topickind_no_key: true when there are NO keys
    );

    ost_ptr as *mut ddsi_sertype
}

/// Free a SampleWrapper's data buffer that was allocated by `sd_to_sample`.
///
/// # Safety
/// The caller must ensure the SampleWrapper was populated by the DDS read path.
/// `sd_to_sample` creates a Vec with capacity == len, so we reconstruct with len as capacity.
pub unsafe fn free_sample_data(wrapper: &mut SampleWrapper) {
    if !wrapper.data.is_null() && wrapper.len > 0 {
        let _ = Vec::from_raw_parts(wrapper.data as *mut u8, wrapper.len, wrapper.len);
        wrapper.data = ptr::null();
        wrapper.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::KeyField;

    #[test]
    fn test_extract_key_fixed_le() {
        // XCDR2 LE payload: encap header [0x00, 0x01, ...] then int32 at offset 4
        let data = [0x00, 0x01, 0x00, 0x00, 0x2A, 0x00, 0x00, 0x00];
        let field = KeyField {
            offset: 4,
            size: 4,
            type_hint: KeyTypeHint::Int32,
        };
        let key = extract_key_bytes(&data, &field);
        assert_eq!(key, &[0x2A, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_extract_key_string_le() {
        // XCDR2 LE: encap [0x00, 0x01, ...], string at offset 4: len=6 "Hello\0"
        let data = [
            0x00, 0x01, 0x00, 0x00, // encap header (LE)
            0x06, 0x00, 0x00, 0x00, // string length = 6 (LE)
            b'H', b'e', b'l', b'l', b'o', 0x00, // "Hello\0"
        ];
        let field = KeyField {
            offset: 4,
            size: 0,
            type_hint: KeyTypeHint::String,
        };
        let key = extract_key_bytes(&data, &field);
        // Should include length prefix + chars
        assert_eq!(key, &[0x06, 0x00, 0x00, 0x00, b'H', b'e', b'l', b'l', b'o', 0x00]);
    }

    #[test]
    fn test_extract_key_string_be() {
        // XCDR2 BE: encap [0x00, 0x00, ...], string at offset 4: len=6 "Hello\0"
        let data = [
            0x00, 0x00, 0x00, 0x00, // encap header (BE)
            0x00, 0x00, 0x00, 0x06, // string length = 6 (BE)
            b'H', b'e', b'l', b'l', b'o', 0x00,
        ];
        let field = KeyField {
            offset: 4,
            size: 0,
            type_hint: KeyTypeHint::String,
        };
        let key = extract_key_bytes(&data, &field);
        assert_eq!(key, &[0x00, 0x00, 0x00, 0x06, b'H', b'e', b'l', b'l', b'o', 0x00]);
    }

    #[test]
    fn test_extract_key_out_of_bounds() {
        let data = [0x00, 0x01];
        let field = KeyField {
            offset: 10,
            size: 4,
            type_hint: KeyTypeHint::Int32,
        };
        assert_eq!(extract_key_bytes(&data, &field), &[]);
    }

    #[test]
    fn test_keys_equal_same() {
        let data = [0x00, 0x01, 0x00, 0x00, 0x2A, 0x00, 0x00, 0x00];
        let fields = vec![KeyField {
            offset: 4,
            size: 4,
            type_hint: KeyTypeHint::Int32,
        }];
        assert!(keys_equal(&data, &data, &fields));
    }

    #[test]
    fn test_keys_equal_different() {
        let a = [0x00, 0x01, 0x00, 0x00, 0x2A, 0x00, 0x00, 0x00];
        let b = [0x00, 0x01, 0x00, 0x00, 0x2B, 0x00, 0x00, 0x00];
        let fields = vec![KeyField {
            offset: 4,
            size: 4,
            type_hint: KeyTypeHint::Int32,
        }];
        assert!(!keys_equal(&a, &b, &fields));
    }

    #[test]
    fn test_hash_key_bytes_deterministic() {
        let data = [0x00, 0x01, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04];
        let fields = vec![KeyField {
            offset: 4,
            size: 4,
            type_hint: KeyTypeHint::Opaque,
        }];
        let h1 = hash_key_bytes(&data, &fields);
        let h2 = hash_key_bytes(&data, &fields);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_key_bytes_differs() {
        let a = [0x00, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00];
        let b = [0x00, 0x01, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00];
        let fields = vec![KeyField {
            offset: 4,
            size: 4,
            type_hint: KeyTypeHint::Int32,
        }];
        assert_ne!(hash_key_bytes(&a, &fields), hash_key_bytes(&b, &fields));
    }

    #[test]
    fn test_keys_equal_empty_fields() {
        // No key fields => always equal
        let a = [1, 2, 3];
        let b = [4, 5, 6];
        assert!(keys_equal(&a, &b, &[]));
    }
}

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
    /// Per-sample serialized key bytes (provided by Go client).
    /// Null if no key bytes were provided (non-keyed or network-received).
    pub key_data: *mut u8,
    pub key_size: usize,
    pub key_capacity: usize,
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
    pub key_data: *const u8,
    pub key_len: usize,
}

impl SampleWrapper {
    /// Create a SampleWrapper that borrows the given slices.
    ///
    /// # Safety
    ///
    /// The returned wrapper holds raw pointers into `data` and `key_bytes`.
    /// The caller must ensure both are not dropped or moved while
    /// the wrapper (or a DDS call using it) is alive.
    pub unsafe fn from_bytes(data: &[u8], key_bytes: &[u8]) -> Self {
        SampleWrapper {
            data: data.as_ptr(),
            len: data.len(),
            key_data: if key_bytes.is_empty() {
                ptr::null()
            } else {
                key_bytes.as_ptr()
            },
            key_len: if key_bytes.is_empty() { 0 } else { key_bytes.len() },
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

/// MurmurHash3 32-bit finalizer, matching CycloneDDS's ddsrt_mh3.
fn murmurhash3(data: &[u8], seed: u32) -> u32 {
    let mut h: u32 = seed;
    let nblocks = data.len() / 4;
    for i in 0..nblocks {
        let mut k = u32::from_le_bytes([
            data[i * 4],
            data[i * 4 + 1],
            data[i * 4 + 2],
            data[i * 4 + 3],
        ]);
        k = k.wrapping_mul(0xcc9e2d51);
        k = k.rotate_left(15);
        k = k.wrapping_mul(0x1b873593);
        h ^= k;
        h = h.rotate_left(13);
        h = h.wrapping_mul(5).wrapping_add(0xe6546b64);
    }
    let tail = &data[nblocks * 4..];
    let mut k1: u32 = 0;
    if tail.len() >= 3 {
        k1 ^= (tail[2] as u32) << 16;
    }
    if tail.len() >= 2 {
        k1 ^= (tail[1] as u32) << 8;
    }
    if !tail.is_empty() {
        k1 ^= tail[0] as u32;
        k1 = k1.wrapping_mul(0xcc9e2d51);
        k1 = k1.rotate_left(15);
        k1 = k1.wrapping_mul(0x1b873593);
        h ^= k1;
    }
    h ^= data.len() as u32;
    h ^= h >> 16;
    h = h.wrapping_mul(0x85ebca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2ae35);
    h ^= h >> 16;
    h
}

/// Compute MD5 hash (for keyhash of keys > 16 bytes).
fn md5_hash(data: &[u8]) -> [u8; 16] {
    use std::num::Wrapping;

    // MD5 constants
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee,
        0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
        0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be,
        0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
        0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa,
        0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
        0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
        0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
        0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05,
        0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039,
        0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1,
        0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
    ];

    let orig_len_bits = (data.len() as u64) * 8;

    // Pad message
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&orig_len_bits.to_le_bytes());

    let (mut a0, mut b0, mut c0, mut d0) = (
        Wrapping(0x67452301u32),
        Wrapping(0xefcdab89u32),
        Wrapping(0x98badcfeu32),
        Wrapping(0x10325476u32),
    );

    for chunk in msg.chunks(64) {
        let mut m = [0u32; 16];
        for (i, m_i) in m.iter_mut().enumerate() {
            *m_i = u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }

        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);

        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let f = f + a + Wrapping(K[i]) + Wrapping(m[g]);
            a = d;
            d = c;
            c = b;
            b = b + Wrapping(f.0.rotate_left(S[i]));
        }

        a0 += a;
        b0 += b;
        c0 += c;
        d0 += d;
    }

    let mut result = [0u8; 16];
    result[0..4].copy_from_slice(&a0.0.to_le_bytes());
    result[4..8].copy_from_slice(&b0.0.to_le_bytes());
    result[8..12].copy_from_slice(&c0.0.to_le_bytes());
    result[12..16].copy_from_slice(&d0.0.to_le_bytes());
    result
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
    let oa = &*(a as *const OpaqueSerdata);
    let ob = &*(b as *const OpaqueSerdata);

    let a_has_key = !oa.key_data.is_null() && oa.key_size > 0;
    let b_has_key = !ob.key_data.is_null() && ob.key_size > 0;

    // If both have per-sample key bytes, compare those directly (memcmp).
    if a_has_key && b_has_key {
        let ka = slice::from_raw_parts(oa.key_data, oa.key_size);
        let kb = slice::from_raw_parts(ob.key_data, ob.key_size);
        return ka == kb;
    }

    // Mixed case: one serdata has key_data (from Go writer) and the other
    // does not (from network receive / keyhash). This can produce incorrect
    // results for APPENDABLE types with variable-offset keys. In practice
    // this is rare: the bridge's primary use-case is writing, and readers
    // on the same topic will typically all have or all lack key_data.
    // We fall through to the key_fields-based comparison as best-effort.

    let st = (*a).type_ as *const OpaqueSertype;
    if (*st).key_fields.is_empty() {
        return true; // keyless topic
    }
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
    if !(*osd).key_data.is_null() && (*osd).key_capacity > 0 {
        let _ = Vec::from_raw_parts((*osd).key_data, (*osd).key_size, (*osd).key_capacity);
    }
    let _ = Box::from_raw(osd);
}

unsafe extern "C" fn sd_from_ser(
    st: *const ddsi_sertype,
    kind: ddsi_serdata_kind,
    fragchain: *const ddsi_rdata,
    size: usize,
) -> *mut ddsi_serdata {
    let mut buf = vec![0u8; size];
    extern "C" {
        fn copy_fragchain_to_buf(fragchain: *const ddsi_rdata, size: usize, buf: *mut u8) -> usize;
    }
    copy_fragchain_to_buf(fragchain, size, buf.as_mut_ptr());

    let (data_ptr, data_len, data_cap) = vec_into_raw(buf);

    let osd = Box::new(OpaqueSerdata {
        base: std::mem::zeroed(),
        data: data_ptr,
        size: data_len,
        capacity: data_cap,
        key_data: ptr::null_mut(),
        key_size: 0,
        key_capacity: 0,
    });
    let osd_ptr = Box::into_raw(osd);
    ddsi_serdata_init(&mut (*osd_ptr).base, st, kind);
    // Network-received serdata: no per-sample key bytes available, so hash
    // falls back to sertype name hash. This differs from Go-writer serdata
    // which uses murmurhash3(key_bytes). The hash is only used for bucket
    // selection in CycloneDDS's concurrent hash table; final equality is
    // determined by sd_eqkey, so correctness is not affected.
    (*osd_ptr).base.hash = st_hash(st);
    osd_ptr as *mut ddsi_serdata
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
        key_data: ptr::null_mut(),
        key_size: 0,
        key_capacity: 0,
    });
    let osd_ptr = Box::into_raw(osd);
    ddsi_serdata_init(&mut (*osd_ptr).base, st, kind);
    // hash MUST be non-zero for CycloneDDS's concurrent hash table (ddsrt_chh).
    // ddsi_serdata_init sets hash=0; CycloneDDS's own CDR sertype sets it to basehash.
    // Use sertype hash as a stable non-zero value.
    (*osd_ptr).base.hash = st_hash(st);
    osd_ptr as *mut ddsi_serdata
}

unsafe extern "C" fn sd_from_keyhash(
    st: *const ddsi_sertype,
    kh: *const ddsi_keyhash,
) -> *mut ddsi_serdata {
    // Copy the 16-byte keyhash into a serdata so CycloneDDS can use it
    // for instance management on keyed topics.
    let kh_bytes = slice::from_raw_parts((*kh).value.as_ptr(), 16);
    let buf = kh_bytes.to_vec();
    let (data_ptr, data_len, data_cap) = vec_into_raw(buf);

    // Do NOT store keyhash as key_data. The keyhash is a hash (or truncated
    // key), not the original key bytes. Storing it in key_data would cause
    // sd_eqkey to compare keyhash bytes against real key bytes from Go
    // writers, producing incorrect results. Leave key_data null so sd_eqkey
    // falls through to the key_fields-based comparison.
    let osd = Box::new(OpaqueSerdata {
        base: std::mem::zeroed(),
        data: data_ptr,
        size: data_len,
        capacity: data_cap,
        key_data: ptr::null_mut(),
        key_size: 0,
        key_capacity: 0,
    });
    let osd_ptr = Box::into_raw(osd);
    ddsi_serdata_init(
        &mut (*osd_ptr).base,
        st,
        ddsi_serdata_kind_SDK_KEY,
    );
    (*osd_ptr).base.hash = st_hash(st);
    osd_ptr as *mut ddsi_serdata
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

    // Copy key bytes if provided
    let (key_ptr, key_len, key_cap) =
        if (*wrapper).key_len > 0 && !(*wrapper).key_data.is_null() {
            let key_src = slice::from_raw_parts((*wrapper).key_data, (*wrapper).key_len);
            vec_into_raw(key_src.to_vec())
        } else {
            (ptr::null_mut(), 0, 0)
        };

    let osd = Box::new(OpaqueSerdata {
        base: std::mem::zeroed(),
        data: data_ptr,
        size: data_len,
        capacity: data_cap,
        key_data: key_ptr,
        key_size: key_len,
        key_capacity: key_cap,
    });
    let osd_ptr = Box::into_raw(osd);
    ddsi_serdata_init(&mut (*osd_ptr).base, st, kind);
    // Use key-derived hash if available, otherwise fallback to sertype name hash
    (*osd_ptr).base.hash = if key_len > 0 {
        let key_bytes = slice::from_raw_parts(key_ptr, key_len);
        murmurhash3(key_bytes, st_hash(st))
    } else {
        st_hash(st)
    };
    osd_ptr as *mut ddsi_serdata
}

unsafe extern "C" fn sd_to_ser(
    sd: *const ddsi_serdata,
    off: usize,
    sz: usize,
    buf: *mut c_void,
) {
    let osd = sd as *const OpaqueSerdata;
    if !(*osd).data.is_null() && off + sz <= (*osd).size && sz > 0 {
        ptr::copy_nonoverlapping((*osd).data.add(off), buf as *mut u8, sz);
    } else if sz > 0 {
        // Zero-fill if data is unavailable to prevent reading uninitialized memory
        ptr::write_bytes(buf as *mut u8, 0, sz);
    }
}

unsafe extern "C" fn sd_to_ser_ref(
    sd: *const ddsi_serdata,
    off: usize,
    sz: usize,
    ref_: *mut ddsrt_iovec_t,
) -> *mut ddsi_serdata {
    let osd = sd as *const OpaqueSerdata;
    if !(*osd).data.is_null() && off + sz <= (*osd).size {
        (*ref_).iov_base = (*osd).data.add(off) as *mut c_void;
        (*ref_).iov_len = sz as _;
    } else if sz > 0 {
        // Out of range (e.g. keyhash serdata with 16 bytes).
        // Allocate zeroed buffer on heap. This leaks, but CycloneDDS
        // requires a valid writable pointer and sd_to_ser_unref only
        // unrefs the serdata, not the iov buffer.
        let buf = vec![0u8; sz].into_boxed_slice();
        (*ref_).iov_base = Box::into_raw(buf) as *mut c_void;
        (*ref_).iov_len = sz as _;
    } else {
        // sz == 0: return any valid pointer
        (*ref_).iov_base = 1usize as *mut c_void; // non-null sentinel
        (*ref_).iov_len = 0;
    }
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
    (*wrapper).key_data = ptr::null();
    (*wrapper).key_len = 0;
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
    force_md5: bool,
) {
    let kh = &mut (*buf).value;
    let osd = sd as *const OpaqueSerdata;

    // Use per-sample key bytes if available
    if !(*osd).key_data.is_null() && (*osd).key_size > 0 {
        let key_bytes = slice::from_raw_parts((*osd).key_data, (*osd).key_size);
        if !force_md5 && key_bytes.len() <= 16 {
            kh[..key_bytes.len()].copy_from_slice(key_bytes);
            kh[key_bytes.len()..].fill(0);
        } else {
            *kh = md5_hash(key_bytes);
        }
        return;
    }

    // Fallback: use key_fields from sertype
    let st = (*sd).type_ as *const OpaqueSertype;
    if (*st).key_fields.is_empty() {
        kh.fill(0);
        return;
    }
    if (*osd).data.is_null() || (*osd).size == 0 {
        kh.fill(0);
        return;
    }
    let data = slice::from_raw_parts((*osd).data, (*osd).size);
    let hash = hash_key_bytes(data, &(*st).key_fields);
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
    from_ser: Some(sd_from_ser),
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
    is_keyed: bool,
    key_descriptors: &KeyDescriptors,
) -> *mut ddsi_sertype {
    let c_name = CString::new(type_name).expect("type_name contains NUL");
    let ost = Box::new(OpaqueSertype {
        base: std::mem::zeroed(),
        key_fields: key_descriptors.keys.clone(),
    });
    let ost_ptr = Box::into_raw(ost);

    // RTPS rule: a NO_KEY reader cannot match a WITH_KEY writer (and vice versa).
    // Use is_keyed from the protocol to set topickind correctly.
    ddsi_sertype_init(
        &mut (*ost_ptr).base,
        c_name.as_ptr(),
        &SERTYPE_OPS_WRAPPER.0,
        &SERDATA_OPS,
        !is_keyed,
    );

    // Set allowed_data_representation to support both XCDR1 (0) and XCDR2 (2).
    // ddsi_sertype_init sets DDS_DATA_REPRESENTATION_RESTRICT_DEFAULT which may
    // exclude XCDR2, causing matching failure with RTI Connext (XCDR2 only).
    // Bit 0 = XCDR1, Bit 2 = XCDR2.
    (*ost_ptr).base.allowed_data_representation = (1u32 << 0) | (1u32 << 2);

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

    // --- MD5 tests (RFC 1321 test vectors) ---

    #[test]
    fn test_md5_empty() {
        let digest = md5_hash(b"");
        assert_eq!(
            digest,
            [0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04,
             0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8, 0x42, 0x7e]
        );
    }

    #[test]
    fn test_md5_abc() {
        let digest = md5_hash(b"abc");
        assert_eq!(
            digest,
            [0x90, 0x01, 0x50, 0x98, 0x3c, 0xd2, 0x4f, 0xb0,
             0xd6, 0x96, 0x3f, 0x7d, 0x28, 0xe1, 0x7f, 0x72]
        );
    }

    #[test]
    fn test_md5_long() {
        // "abcdefghijklmnopqrstuvwxyz"
        let digest = md5_hash(b"abcdefghijklmnopqrstuvwxyz");
        assert_eq!(
            digest,
            [0xc3, 0xfc, 0xd3, 0xd7, 0x61, 0x92, 0xe4, 0x00,
             0x7d, 0xfb, 0x49, 0x6c, 0xca, 0x67, 0xe1, 0x3b]
        );
    }

    // --- MurmurHash3 tests ---

    #[test]
    fn test_murmurhash3_deterministic() {
        let data = [0x01, 0x02, 0x03, 0x04];
        let h1 = murmurhash3(&data, 0);
        let h2 = murmurhash3(&data, 0);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_murmurhash3_different_data() {
        let a = [0x01, 0x02, 0x03, 0x04];
        let b = [0x05, 0x06, 0x07, 0x08];
        assert_ne!(murmurhash3(&a, 0), murmurhash3(&b, 0));
    }

    #[test]
    fn test_murmurhash3_different_seeds() {
        let data = [0x01, 0x02, 0x03, 0x04];
        assert_ne!(murmurhash3(&data, 0), murmurhash3(&data, 42));
    }

    #[test]
    fn test_murmurhash3_empty() {
        // Empty data should not panic, and should produce seed-dependent output
        let h = murmurhash3(&[], 0);
        assert_eq!(h, murmurhash3(&[], 0));
        assert_ne!(murmurhash3(&[], 0), murmurhash3(&[], 1));
    }

    #[test]
    fn test_murmurhash3_tail() {
        // Non-multiple-of-4 length exercises the tail handling
        let data = [0x01, 0x02, 0x03, 0x04, 0x05];
        let h = murmurhash3(&data, 0);
        assert_ne!(h, 0);
    }
}

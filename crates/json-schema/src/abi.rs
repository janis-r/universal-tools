//! Host-neutral exports for the shared WebAssembly module.
//!
//! JavaScript, Python, and Go load the same `ut_json_schema.wasm` file. Strings
//! cross the boundary as pointer plus length. A compiled schema is an integer
//! handle. The only import is `ut.now_micros`, a host clock in microseconds.
//!
//! A returned buffer starts with a little-endian `u32` byte length, followed by
//! the UTF-8 payload. The caller reads those bytes and then calls
//! [`ut_free_result`]. Input buffers allocated with [`ut_alloc`] are released
//! with [`ut_dealloc`].

use std::alloc::Layout;
use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::{json, Value};

use crate::engine::{self, Prepared};

struct Store {
    next: u32,
    schemas: HashMap<u32, Prepared>,
}

thread_local! {
    static STORE: RefCell<Store> = RefCell::new(Store {
        next: 1,
        schemas: HashMap::new(),
    });
}

#[no_mangle]
pub extern "C" fn ut_alloc(len: u32) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }
    let layout = Layout::from_size_align(len as usize, 1).expect("input layout");
    unsafe { std::alloc::alloc(layout) }
}

/// # Safety
/// `ptr` must be a pointer returned by [`ut_alloc`] for `len` bytes, or null.
#[no_mangle]
pub unsafe extern "C" fn ut_dealloc(ptr: *mut u8, len: u32) {
    if ptr.is_null() || len == 0 {
        return;
    }
    let layout = Layout::from_size_align(len as usize, 1).expect("input layout");
    unsafe { std::alloc::dealloc(ptr, layout) }
}

/// # Safety
/// `ptr` must be a pointer returned by an export that writes a result buffer, or null.
#[no_mangle]
pub unsafe extern "C" fn ut_free_result(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let payload = unsafe { (ptr as *const u32).read() } as usize;
    let Some(layout) = result_layout(payload) else {
        return;
    };
    unsafe { std::alloc::dealloc(ptr, layout) }
}

/// # Safety
/// `ptr` must be null or address `len` readable bytes in this module's memory.
#[no_mangle]
pub unsafe extern "C" fn ut_execute(ptr: *const u8, len: u32) -> *mut u8 {
    let request = match text(ptr, len) {
        Ok(request) => request,
        Err(message) => return pack(super::fallback_response(&message).to_string().as_bytes()),
    };
    pack(crate::execute_json(&request).as_bytes())
}

/// # Safety
/// Each pointer must be null or address its length in this module's memory.
#[no_mangle]
pub unsafe extern "C" fn ut_compile(
    schema_ptr: *const u8,
    schema_len: u32,
    pointer_ptr: *const u8,
    pointer_len: u32,
    draft_ptr: *const u8,
    draft_len: u32,
) -> *mut u8 {
    let json = match (
        text(schema_ptr, schema_len),
        text(pointer_ptr, pointer_len),
        text(draft_ptr, draft_len),
    ) {
        (Ok(schema), Ok(pointer), Ok(draft)) => {
            catch_unwind(AssertUnwindSafe(|| compile_json(&schema, &pointer, &draft)))
        }
        (Err(message), _, _) | (_, Err(message), _) | (_, _, Err(message)) => {
            return pack(panic_compile(&message).as_bytes());
        }
    };
    match json {
        Ok(body) => pack(body.as_bytes()),
        Err(payload) => pack(panic_compile(&super::panic_message(payload)).as_bytes()),
    }
}

/// # Safety
/// `ptr` must be null or address `len` readable bytes in this module's memory.
#[no_mangle]
pub unsafe extern "C" fn ut_validate(handle: u32, ptr: *const u8, len: u32) -> *mut u8 {
    let instance = match text(ptr, len) {
        Ok(instance) => instance,
        Err(message) => return pack(super::fallback_response(&message).to_string().as_bytes()),
    };
    let response = catch_unwind(AssertUnwindSafe(|| validate_json(handle, &instance)));
    match response {
        Ok(body) => pack(body.as_bytes()),
        Err(payload) => pack(super::fallback_response(&super::panic_message(payload)).to_string().as_bytes()),
    }
}

/// # Safety
/// `ptr` must be null or address `len` readable bytes in this module's memory.
#[no_mangle]
pub unsafe extern "C" fn ut_spawn(handle: u32, ptr: *const u8, len: u32) -> *mut u8 {
    let pointer = match text(ptr, len) {
        Ok(pointer) => pointer,
        Err(message) => return pack(panic_compile(&message).as_bytes()),
    };
    let spawned = catch_unwind(AssertUnwindSafe(|| spawn_json(handle, &pointer)));
    match spawned {
        Ok(body) => pack(body.as_bytes()),
        Err(payload) => pack(panic_compile(&super::panic_message(payload)).as_bytes()),
    }
}

#[no_mangle]
pub extern "C" fn ut_free(handle: u32) {
    STORE.with(|store| {
        store.borrow_mut().schemas.remove(&handle);
    });
}

fn compile_json(schema_json: &str, pointer: &str, draft: &str) -> String {
    let override_draft = (!draft.is_empty()).then_some(draft);
    let schema = match serde_json::from_str::<Value>(schema_json) {
        Ok(value) => value,
        Err(error) => {
            return json!({
                "draft": override_draft.unwrap_or("draft-07"),
                "initError": crate::errors::init_error(format!("schema is not JSON: {error}")),
            })
            .to_string();
        }
    };
    let budget = ut_contract::ResourceBudget::default();
    let (prepared, sample) = ut_contract::measure(|| engine::prepare(&schema, pointer, override_draft));
    if !sample.within_budget(budget) {
        return json!({
            "draft": "draft-07",
            "failureMessage": format!(
                "execution used {}us and {} peak bytes; budget is {}us and {} bytes",
                sample.elapsed_micros, sample.peak_bytes, budget.max_elapsed_micros, budget.max_peak_bytes
            ),
        })
        .to_string();
    }
    match prepared {
        Ok(prepared) => {
            let draft = prepared.draft.clone();
            let handle = insert(prepared);
            json!({ "draft": draft, "handle": handle }).to_string()
        }
        Err(failure) => json!({
            "draft": failure.draft,
            "initError": failure.error,
        })
        .to_string(),
    }
}

fn validate_json(handle: u32, instance_json: &str) -> String {
    let found = STORE.with(|store| {
        let store = store.borrow();
        store.schemas.get(&handle).map(|prepared| {
            super::validate_prepared(prepared, instance_json).to_string()
        })
    });
    found.unwrap_or_else(|| super::fallback_response("compiled validator was released").to_string())
}

fn spawn_json(handle: u32, pointer: &str) -> String {
    let spawned = STORE.with(|store| {
        let store = store.borrow();
        store.schemas.get(&handle).map(|prepared| prepared.spawn(pointer))
    });
    match spawned {
        None => released_compile(),
        Some(Ok(prepared)) => {
            let draft = prepared.draft.clone();
            let handle = insert(prepared);
            json!({ "draft": draft, "handle": handle }).to_string()
        }
        Some(Err(failure)) => json!({
            "draft": failure.draft,
            "initError": failure.error,
        })
        .to_string(),
    }
}

fn insert(prepared: Prepared) -> u32 {
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        loop {
            let handle = store.next;
            store.next = store.next.wrapping_add(1);
            if store.next == 0 {
                store.next = 1;
            }
            if handle != 0 && !store.schemas.contains_key(&handle) {
                store.schemas.insert(handle, prepared);
                return handle;
            }
        }
    })
}

fn released_compile() -> String {
    json!({
        "draft": "draft-07",
        "initError": crate::errors::init_error("compiled validator was released"),
    })
    .to_string()
}

fn panic_compile(message: &str) -> String {
    json!({
        "draft": "draft-07",
        "initError": crate::errors::init_error(message),
    })
    .to_string()
}

fn text(ptr: *const u8, len: u32) -> Result<String, String> {
    if len == 0 {
        return Ok(String::new());
    }
    if ptr.is_null() {
        return Err("null pointer".to_owned());
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text.to_owned()),
        Err(error) => Err(format!("text is not UTF-8: {error}")),
    }
}

fn pack(bytes: &[u8]) -> *mut u8 {
    let Some(layout) = result_layout(bytes.len()) else {
        return std::ptr::null_mut();
    };
    let ptr = unsafe { std::alloc::alloc(layout) };
    if ptr.is_null() {
        return ptr;
    }
    unsafe {
        (ptr as *mut u32).write(bytes.len() as u32);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.add(4), bytes.len());
    }
    ptr
}

fn result_layout(payload: usize) -> Option<Layout> {
    let total = payload.checked_add(4)?;
    let padded = total.checked_add(3)? & !3;
    Layout::from_size_align(padded, 4).ok()
}

/// Entropy for crates that hash with a random seed. The bytes only need to
/// vary between calls. They are not a secret, and they do not come from JavaScript.
#[no_mangle]
unsafe extern "Rust" fn __getrandom_v03_custom(dest: *mut u8, len: usize) -> Result<(), getrandom::Error> {
    use std::cell::Cell;

    thread_local! {
        static STATE: Cell<u64> = const { Cell::new(0xA076_1D64_78BD_642F) };
    }
    if len == 0 || dest.is_null() {
        return Ok(());
    }
    STATE.with(|state| {
        let mut value = state.get();
        let bytes = unsafe { std::slice::from_raw_parts_mut(dest, len) };
        for byte in bytes {
            value = value.wrapping_mul(0x6C62_272E_07BB_0142).wrapping_add(1);
            *byte = (value >> 33) as u8;
        }
        state.set(value);
    });
    Ok(())
}

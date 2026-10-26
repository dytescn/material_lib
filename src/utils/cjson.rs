use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::ffi::{c_char, c_int, CStr};

/// 与 database.dll / cyctron 保持一致的输出缓冲上限。
pub const MAX_OUTPUT_LEN: usize = 65536;

pub fn parse_input<T: DeserializeOwned>(ptr: *const c_char) -> Result<T, (i32, String)> {
    if ptr.is_null() {
        return Err((400100, "json_in is null".to_string()));
    }
    let body = unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|e| (400101, format!("invalid utf-8: {}", e)))?;
    if body.trim().is_empty() {
        return Ok(serde_json::from_str("{}").unwrap());
    }
    serde_json::from_str::<T>(body).map_err(|e| (400102, format!("invalid json: {}", e)))
}

pub fn write_json_out(json_out: *mut c_char, value: Value) -> c_int {
    if json_out.is_null() {
        return -1;
    }
    let text = value.to_string();
    let bytes = text.as_bytes();
    if bytes.len() + 1 > MAX_OUTPUT_LEN {
        let fallback = r#"{"code":500102,"data":{},"msg":"response exceeds 64KB"}"#;
        unsafe {
            std::ptr::copy_nonoverlapping(fallback.as_ptr(), json_out as *mut u8, fallback.len());
            *json_out.add(fallback.len()) = 0;
        }
        return -2;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), json_out as *mut u8, bytes.len());
        *json_out.add(bytes.len()) = 0;
    }
    0
}

pub fn write_success(json_out: *mut c_char, data: Value) -> c_int {
    write_json_out(
        json_out,
        json!({ "code": 200, "data": data, "msg": "success" }),
    )
}

pub fn write_error(json_out: *mut c_char, code: i32, msg: &str) -> c_int {
    write_json_out(
        json_out,
        json!({ "code": code, "data": {}, "msg": msg }),
    )
}

/// 统一的 FFI 函数外壳：把业务错误转成 JSON 响应。
pub fn handle<F>(_json_in: *const c_char, json_out: *mut c_char, f: F) -> c_int
where
    F: FnOnce() -> Result<Value, (i32, String)>,
{
    match f() {
        Ok(data) => write_success(json_out, data),
        Err((code, msg)) => write_error(json_out, code, &msg),
    }
}

/// 允许请求体为空（如 setup）。
pub fn parse_optional<T: DeserializeOwned + Default>(
    ptr: *const c_char,
) -> Result<T, (i32, String)> {
    if ptr.is_null() {
        return Ok(T::default());
    }
    let body = unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|e| (400101, format!("invalid utf-8: {}", e)))?;
    if body.trim().is_empty() {
        return Ok(T::default());
    }
    serde_json::from_str::<T>(body).map_err(|e| (400102, format!("invalid json: {}", e)))
}

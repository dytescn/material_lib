use libloading::Library;
use serde_json::Value;
use std::ffi::{c_char, c_int, CStr, CString};
use std::sync::OnceLock;

pub const MATERIAL_DB: &str = "material";
pub const ROUTER_DB: &str = "./router";

/// database 1.0.5+ 约定 `json_out` 至少 1 MiB。
const OUT_BUF_SIZE: usize = 1024 * 1024;

#[cfg(windows)]
const DATABASE_LIB: &str = "./libary/database.dll";
#[cfg(target_os = "linux")]
const DATABASE_LIB: &str = "./libary/database.so";
#[cfg(target_os = "macos")]
const DATABASE_LIB: &str = "./libary/database.dylib";

type FfiFn = unsafe extern "C" fn(*const c_char, *mut c_char) -> c_int;

static LIB: OnceLock<Library> = OnceLock::new();

fn library() -> Result<&'static Library, (i32, String)> {
    if let Some(lib) = LIB.get() {
        return Ok(lib);
    }
    let lib = unsafe { Library::new(DATABASE_LIB) }
        .map_err(|e| (500100, format!("load {} failed: {}", DATABASE_LIB, e)))?;
    let _ = LIB.set(lib);
    Ok(LIB.get().unwrap())
}

fn call(symbol: &[u8], body: Value) -> Result<Value, (i32, String)> {
    let lib = library()?;
    let func: libloading::Symbol<'_, FfiFn> = unsafe {
        lib.get(symbol)
            .map_err(|e| (500101, format!("dlsym failed: {}", e)))?
    };

    let req = CString::new(body.to_string())
        .map_err(|_| (400102, "request json contains \\0".to_string()))?;
    let mut out = vec![0u8; OUT_BUF_SIZE];
    let ret = unsafe { func(req.as_ptr(), out.as_mut_ptr() as *mut c_char) };

    let text = unsafe { CStr::from_ptr(out.as_ptr() as *const c_char) }
        .to_str()
        .map_err(|e| (500101, format!("invalid utf-8 response: {}", e)))?;
    if text.is_empty() {
        return Err((500101, format!("database returned empty (ret={})", ret)));
    }

    let value: Value = serde_json::from_str(text)
        .map_err(|e| (500101, format!("invalid response: {} ({})", e, text)))?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 200 && code != 0 {
        let msg = value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("unknown database error")
            .to_string();
        return Err(map_error(&msg));
    }
    Ok(value)
}

fn map_error(msg: &str) -> (i32, String) {
    if msg.contains("UNIQUE constraint failed") {
        return (409100, msg.to_string());
    }
    if msg.contains("no such table") {
        return (500201, format!("NEED_SETUP: {msg}"));
    }
    (500101, msg.to_string())
}

pub fn query(db: &str, sql: &str) -> Result<Vec<Value>, (i32, String)> {
    let value = call(
        b"db_query",
        serde_json::json!({ "path": db, "sql": sql }),
    )?;
    Ok(value
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

pub fn query_count(db: &str, sql: &str) -> Result<i64, (i32, String)> {
    let rows = query(db, sql)?;
    Ok(rows
        .first()
        .and_then(|row| row.get("n"))
        .and_then(Value::as_i64)
        .unwrap_or(0))
}

pub fn execute(symbol: &[u8], db: &str, sql: &str) -> Result<(), (i32, String)> {
    call(symbol, serde_json::json!({ "path": db, "sql": sql }))?;
    Ok(())
}

pub fn update(db: &str, sql: &str) -> Result<(), (i32, String)> {
    execute(b"db_update", db, sql)
}

pub fn insert(db: &str, sql: &str) -> Result<(), (i32, String)> {
    execute(b"db_insert", db, sql)
}

pub fn delete(db: &str, sql: &str) -> Result<(), (i32, String)> {
    execute(b"db_delete", db, sql)
}

pub fn create_table(db: &str, table: &str, schema: &str) -> Result<(), (i32, String)> {
    call(
        b"db_create",
        serde_json::json!({ "path": db, "table": table, "schema": schema }),
    )?;
    Ok(())
}

/// SQL 字符串字面量转义。
pub fn esc(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub fn opt_str(value: &Option<String>) -> String {
    match value {
        Some(v) if !v.is_empty() => esc(v),
        _ => "NULL".to_string(),
    }
}

pub fn now_sql() -> &'static str {
    "datetime('now','localtime')"
}

/// 从 JSON 对象中取字符串。
pub fn get_str<'a>(obj: &'a Value, key: &str) -> &'a str {
    obj.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn get_i64(obj: &Value, key: &str) -> i64 {
    obj.get(key).and_then(Value::as_i64).unwrap_or(0)
}

pub fn get_opt_string(obj: &Value, key: &str) -> Option<String> {
    obj.get(key).and_then(Value::as_str).map(str::to_string)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_table_maps_to_need_setup() {
        let (code, msg) = map_error("no such table: material_file");
        assert_eq!(code, 500201);
        assert!(msg.starts_with("NEED_SETUP:"));
    }
}

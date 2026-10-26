use libloading::Library;
use serde_json::{json, Value};
use std::ffi::{c_char, c_int, CStr, CString};
use std::sync::OnceLock;

const CHATLLM_LIB: &str = "./libary/chatllm.dll";
const OUT_BUF_SIZE: usize = 65536;

type FfiFn = unsafe extern "C" fn(*const c_char, *mut c_char) -> c_int;

static LIB: OnceLock<Library> = OnceLock::new();

fn library() -> Result<&'static Library, (i32, String)> {
    if let Some(lib) = LIB.get() {
        return Ok(lib);
    }
    let lib = unsafe { Library::new(CHATLLM_LIB) }
        .map_err(|e| (500103, format!("load {} failed: {}", CHATLLM_LIB, e)))?;
    let _ = LIB.set(lib);
    Ok(LIB.get().unwrap())
}

fn call(symbol: &[u8], body: Value) -> Result<Value, (i32, String)> {
    let lib = library()?;
    let func: libloading::Symbol<'_, FfiFn> = unsafe {
        lib.get(symbol)
            .map_err(|e| (500103, format!("dlsym failed: {}", e)))?
    };

    let req = CString::new(body.to_string())
        .map_err(|_| (400102, "request json contains \\0".to_string()))?;
    let mut out = vec![0u8; OUT_BUF_SIZE];
    let _ret = unsafe { func(req.as_ptr(), out.as_mut_ptr() as *mut c_char) };

    let text = unsafe { CStr::from_ptr(out.as_ptr() as *const c_char) }
        .to_str()
        .map_err(|e| (500103, format!("invalid utf-8 response: {}", e)))?;
    if text.is_empty() {
        return Err((500103, "chatllm returned empty".to_string()));
    }
    let value: Value = serde_json::from_str(text)
        .map_err(|e| (500103, format!("invalid response: {} ({})", e, text)))?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 200 && code != 0 {
        let msg = value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("chatllm error")
            .to_string();
        return Err((500103, msg));
    }
    Ok(value.get("data").cloned().unwrap_or_else(|| json!({})))
}

pub fn material_search(
    query: &str,
    top_k: i64,
    min_score: f32,
) -> Result<Value, (i32, String)> {
    call(
        b"post_material_search",
        json!({
            "query": query,
            "top_k": top_k,
            "min_score": min_score,
        }),
    )
}

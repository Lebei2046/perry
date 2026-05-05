//! Native bindings for the npm `axios` HTTP client.
//!
//! Phase 5 step 13 — first HTTP-client wrapper port. Uses
//! perry-ffi v0.5.x's full surface: handle registry +
//! spawn_blocking + JsPromise + JsValue. Reqwest under the hood
//! (same as perry-stdlib's existing axios copy).
//!
//! Functionally identical to `crates/perry-stdlib/src/axios.rs`.

use perry_ffi::{
    alloc_string, get_handle, read_string, register_handle, spawn_blocking, with_handle, Handle,
    JsPromise, JsString, JsValue, Promise, StringHeader,
};

/// Response handle wrapper.
pub struct AxiosResponseHandle {
    pub status: u16,
    pub status_text: String,
    pub data: String,
}

unsafe fn read_str(ptr: *const StringHeader) -> Option<String> {
    let handle = JsString::from_raw(ptr as *mut StringHeader);
    read_string(handle).map(String::from)
}

/// Common request driver — runs the reqwest call inside
/// spawn_blocking, packages the response into an
/// `AxiosResponseHandle`, registers it, and resolves the promise
/// with a POINTER_TAG-tagged handle value (issue #340 trick from
/// the original perry-stdlib axios — without the explicit
/// NaN-boxing, the awaiter sees a subnormal float that decays
/// to `undefined` on `r.status` accesses).
fn run_request<F>(method: &'static str, url_or_err: Result<String, &'static str>, build: F) -> *mut Promise
where
    F: FnOnce(reqwest::Client, String) -> reqwest::RequestBuilder + Send + 'static,
{
    let promise = JsPromise::new();
    let raw = promise.as_raw();
    let url = match url_or_err {
        Ok(u) => u,
        Err(msg) => {
            promise.reject_string(msg);
            return raw;
        }
    };

    spawn_blocking(move || {
        let result: Result<AxiosResponseHandle, String> =
            tokio::runtime::Handle::current().block_on(async move {
                let client = reqwest::Client::new();
                let request = build(client, url);
                let response = request
                    .send()
                    .await
                    .map_err(|e| format!("{} request failed: {}", method, e))?;
                let status = response.status().as_u16();
                let status_text = response
                    .status()
                    .canonical_reason()
                    .unwrap_or("")
                    .to_string();
                let data = response
                    .text()
                    .await
                    .map_err(|e| format!("Failed to read response body: {}", e))?;
                Ok(AxiosResponseHandle {
                    status,
                    status_text,
                    data,
                })
            });
        match result {
            Ok(resp) => {
                let handle = register_handle(resp);
                // POINTER_TAG-tagged handle value — see #340.
                promise.resolve(JsValue::from_object_ptr(handle as *mut ()));
            }
            Err(msg) => promise.reject_string(&msg),
        }
    });
    raw
}

/// `axios.get(url) -> Promise<Response>`.
///
/// # Safety
///
/// `url_ptr` must be null or a Perry-runtime `StringHeader`.
#[no_mangle]
pub unsafe extern "C" fn js_axios_get(url_ptr: *const StringHeader) -> *mut Promise {
    let url = read_str(url_ptr).ok_or("Invalid URL");
    run_request("GET", url, |client, url| client.get(&url))
}

/// `axios.post(url, data) -> Promise<Response>`.
///
/// # Safety
///
/// Both pointers must be null or Perry-runtime `StringHeader`s.
#[no_mangle]
pub unsafe extern "C" fn js_axios_post(
    url_ptr: *const StringHeader,
    data_ptr: *const StringHeader,
) -> *mut Promise {
    let url = read_str(url_ptr).ok_or("Invalid URL");
    let body = read_str(data_ptr).unwrap_or_default();
    run_request("POST", url, move |client, url| {
        client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(body)
    })
}

/// `axios.put(url, data) -> Promise<Response>`.
///
/// # Safety
///
/// Both pointers must be null or Perry-runtime `StringHeader`s.
#[no_mangle]
pub unsafe extern "C" fn js_axios_put(
    url_ptr: *const StringHeader,
    data_ptr: *const StringHeader,
) -> *mut Promise {
    let url = read_str(url_ptr).ok_or("Invalid URL");
    let body = read_str(data_ptr).unwrap_or_default();
    run_request("PUT", url, move |client, url| {
        client
            .put(&url)
            .header("Content-Type", "application/json")
            .body(body)
    })
}

/// `axios.delete(url) -> Promise<Response>`.
///
/// # Safety
///
/// `url_ptr` must be null or a Perry-runtime `StringHeader`.
#[no_mangle]
pub unsafe extern "C" fn js_axios_delete(url_ptr: *const StringHeader) -> *mut Promise {
    let url = read_str(url_ptr).ok_or("Invalid URL");
    run_request("DELETE", url, |client, url| client.delete(&url))
}

/// `axios.patch(url, data) -> Promise<Response>`.
///
/// # Safety
///
/// Both pointers must be null or Perry-runtime `StringHeader`s.
#[no_mangle]
pub unsafe extern "C" fn js_axios_patch(
    url_ptr: *const StringHeader,
    data_ptr: *const StringHeader,
) -> *mut Promise {
    let url = read_str(url_ptr).ok_or("Invalid URL");
    let body = read_str(data_ptr).unwrap_or_default();
    run_request("PATCH", url, move |client, url| {
        client
            .patch(&url)
            .header("Content-Type", "application/json")
            .body(body)
    })
}

/// `response.status -> number`.
#[no_mangle]
pub extern "C" fn js_axios_response_status(handle: Handle) -> f64 {
    if let Some(r) = get_handle::<AxiosResponseHandle>(handle) {
        r.status as f64
    } else {
        0.0
    }
}

/// `response.statusText -> string`.
#[no_mangle]
pub extern "C" fn js_axios_response_status_text(handle: Handle) -> *mut StringHeader {
    with_handle::<AxiosResponseHandle, _, _>(handle, |r| alloc_string(&r.status_text).as_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// `response.data -> string`.
#[no_mangle]
pub extern "C" fn js_axios_response_data(handle: Handle) -> *mut StringHeader {
    with_handle::<AxiosResponseHandle, _, _>(handle, |r| alloc_string(&r.data).as_raw())
        .unwrap_or(std::ptr::null_mut())
}

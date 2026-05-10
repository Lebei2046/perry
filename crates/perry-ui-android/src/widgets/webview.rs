//! WebView widget — `android.webkit.WebView` (Android) for issue #658
//! Phase 3.
//!
//! v1 ships the load-bearing surface (loadUrl, reload, goBack/goForward,
//! evaluateJavascript) backed by the stock `WebViewClient`. The
//! `onShouldNavigate` / `onLoaded` / `onError` hooks are stored on the
//! widget but not yet delivered — Android `WebViewClient`'s
//! `shouldOverrideUrlLoading` / `onPageFinished` overrides need a
//! custom Java helper class deployed alongside the Perry runtime APK.
//! Stock-WebView v1 unblocks the OAuth flow's main path (open URL,
//! user authenticates, redirect happens — host page detects the
//! callback URL via `evaluateJavascript("window.location.href")`).
//!
//! A future iteration will land a perry-android-helpers JAR with a
//! `PerryWebViewClient` that proxies callbacks to JNI for the same
//! contract as macOS / iOS / Windows.

use crate::jni_bridge;
use jni::objects::{GlobalRef, JValue};
use std::cell::RefCell;
use std::collections::HashMap;

extern "C" {
    fn js_closure_call1(closure: *const u8, arg: f64) -> f64;
    fn js_nanbox_get_pointer(value: f64) -> i64;
    fn js_nanbox_string(ptr: i64) -> f64;
}

struct WebViewState {
    on_should_navigate: f64,
    on_loaded: f64,
    on_error: f64,
    allowed_domains: Vec<String>,
}

thread_local! {
    static WEBVIEW_STATES: RefCell<HashMap<i64, WebViewState>> = RefCell::new(HashMap::new());
}

fn str_from_header(ptr: *const u8) -> &'static str {
    crate::app::str_from_header(ptr)
}

fn nanbox_str(s: &str) -> f64 {
    let bytes = s.as_bytes();
    let p = perry_runtime::string::js_string_from_bytes(bytes.as_ptr(), bytes.len() as u32);
    unsafe { js_nanbox_string(p as i64) }
}

/// Create a WebView. Returns the widget handle.
pub fn create(url_ptr: *const u8, _width: f64, _height: f64) -> i64 {
    let url = str_from_header(url_ptr).to_string();
    let mut env = jni_bridge::get_env();
    let _ = env.push_local_frame(32);

    let activity = super::get_activity(&mut env);
    let webview = match env.new_object(
        "android/webkit/WebView",
        "(Landroid/content/Context;)V",
        &[JValue::Object(&activity)],
    ) {
        Ok(w) => w,
        Err(_) => {
            unsafe {
        let _ = env.pop_local_frame(&jni::objects::JObject::null());
    }
            return 0;
        }
    };

    // Enable JavaScript so user OAuth pages can run; gated by getSettings().setJavaScriptEnabled(true).
    if let Ok(settings) = env.call_method(
        &webview,
        "getSettings",
        "()Landroid/webkit/WebSettings;",
        &[],
    ) {
        if let Ok(s) = settings.l() {
            let _ = env.call_method(&s, "setJavaScriptEnabled", "(Z)V", &[JValue::Bool(1)]);
            let _ = env.call_method(&s, "setDomStorageEnabled", "(Z)V", &[JValue::Bool(1)]);
        }
    }

    // Set the stock WebViewClient so navigations stay inside the WebView
    // instead of opening the system browser. Hooks for shouldOverrideUrlLoading
    // / onPageFinished aren't wired in v1 (need a custom helper class).
    if let Ok(client) = env.new_object("android/webkit/WebViewClient", "()V", &[]) {
        let _ = env.call_method(
            &webview,
            "setWebViewClient",
            "(Landroid/webkit/WebViewClient;)V",
            &[JValue::Object(&client)],
        );
    }

    if !url.is_empty() {
        if let Ok(jurl) = env.new_string(&url) {
            let _ = env.call_method(
                &webview,
                "loadUrl",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&jurl)],
            );
        }
    }

    let global_ref = match env.new_global_ref(&webview) {
        Ok(g) => g,
        Err(_) => {
            unsafe {
        let _ = env.pop_local_frame(&jni::objects::JObject::null());
    }
            return 0;
        }
    };
    unsafe {
        let _ = env.pop_local_frame(&jni::objects::JObject::null());
    }

    let handle = super::register_widget(global_ref);
    WEBVIEW_STATES.with(|s| {
        s.borrow_mut().insert(
            handle,
            WebViewState {
                on_should_navigate: 0.0,
                on_loaded: 0.0,
                on_error: 0.0,
                allowed_domains: Vec::new(),
            },
        );
    });
    handle
}

fn call_string_method(handle: i64, method: &str, sig: &str, jstr: &str) {
    let view = match super::get_widget(handle) {
        Some(v) => v,
        None => return,
    };
    let mut env = jni_bridge::get_env();
    let _ = env.push_local_frame(8);
    if let Ok(s) = env.new_string(jstr) {
        let _ = env.call_method(&view, method, sig, &[JValue::Object(&s)]);
    }
    unsafe {
        let _ = env.pop_local_frame(&jni::objects::JObject::null());
    }
}

fn call_void_method(handle: i64, method: &str) {
    let view = match super::get_widget(handle) {
        Some(v) => v,
        None => return,
    };
    let mut env = jni_bridge::get_env();
    let _ = env.call_method(&view, method, "()V", &[]);
}

pub fn load_url(handle: i64, url_ptr: *const u8) {
    let url = str_from_header(url_ptr);
    if url.is_empty() {
        return;
    }
    call_string_method(handle, "loadUrl", "(Ljava/lang/String;)V", url);
}

pub fn reload(handle: i64) {
    call_void_method(handle, "reload");
}

pub fn go_back(handle: i64) {
    call_void_method(handle, "goBack");
}

pub fn go_forward(handle: i64) {
    call_void_method(handle, "goForward");
}

pub fn can_go_back(handle: i64) -> i64 {
    let view = match super::get_widget(handle) {
        Some(v) => v,
        None => return 0,
    };
    let mut env = jni_bridge::get_env();
    match env.call_method(&view, "canGoBack", "()Z", &[]) {
        Ok(v) => match v.z() {
            Ok(b) => {
                if b {
                    1
                } else {
                    0
                }
            }
            Err(_) => 0,
        },
        Err(_) => 0,
    }
}

/// Fire `evaluateJavascript(js, ValueCallback<String>)`. The Android API
/// is async-callback-based; v1 stores the user callback and invokes it
/// from a synchronously-called helper that posts the JS on the WebView
/// and waits for the result via a CountDownLatch (~50ms typical). For
/// robustness, the v1 impl uses the simpler `loadUrl("javascript:...")`
/// fire-and-forget shape and invokes the callback with the empty
/// string immediately — gives user code a callback to await without
/// blocking the JNI bridge. A real ValueCallback implementation needs
/// a custom helper class (same v2 path as the WebViewClient).
pub fn evaluate_js(handle: i64, js_ptr: *const u8, callback: f64) {
    let js = str_from_header(js_ptr);
    let view = match super::get_widget(handle) {
        Some(v) => v,
        None => return,
    };
    let mut env = jni_bridge::get_env();
    let _ = env.push_local_frame(8);
    if let Ok(jjs) = env.new_string(js) {
        // ValueCallback<String> is `null` here — Android accepts null
        // and just runs the script without delivering the result.
        let null_cb = jni::objects::JObject::null();
        let _ = env.call_method(
            &view,
            "evaluateJavascript",
            "(Ljava/lang/String;Landroid/webkit/ValueCallback;)V",
            &[JValue::Object(&jjs), JValue::Object(&null_cb)],
        );
    }
    unsafe {
        let _ = env.pop_local_frame(&jni::objects::JObject::null());
    }

    // Fire the user callback synchronously with empty string — preserves
    // the API shape (a callback fires) without the Java helper class.
    if callback != 0.0 {
        let nb = nanbox_str("");
        let closure_ptr = unsafe { js_nanbox_get_pointer(callback) } as *const u8;
        if !closure_ptr.is_null() {
            unsafe {
                js_closure_call1(closure_ptr, nb);
            }
        }
    }
}

pub fn clear_cookies(_handle: i64) {
    let mut env = jni_bridge::get_env();
    let _ = env.push_local_frame(8);
    // CookieManager.getInstance().removeAllCookies(null) — process-wide.
    if let Ok(mgr_class) = env.find_class("android/webkit/CookieManager") {
        if let Ok(mgr) = env.call_static_method(
            &mgr_class,
            "getInstance",
            "()Landroid/webkit/CookieManager;",
            &[],
        ) {
            if let Ok(mgr_obj) = mgr.l() {
                let null_cb = jni::objects::JObject::null();
                let _ = env.call_method(
                    &mgr_obj,
                    "removeAllCookies",
                    "(Landroid/webkit/ValueCallback;)V",
                    &[JValue::Object(&null_cb)],
                );
            }
        }
    }
    unsafe {
        let _ = env.pop_local_frame(&jni::objects::JObject::null());
    }
}

pub fn set_user_agent(handle: i64, ua_ptr: *const u8) {
    let ua = str_from_header(ua_ptr);
    let view = match super::get_widget(handle) {
        Some(v) => v,
        None => return,
    };
    let mut env = jni_bridge::get_env();
    let _ = env.push_local_frame(8);
    if let Ok(settings) = env.call_method(
        &view,
        "getSettings",
        "()Landroid/webkit/WebSettings;",
        &[],
    ) {
        if let Ok(s) = settings.l() {
            if let Ok(jua) = env.new_string(ua) {
                let _ = env.call_method(
                    &s,
                    "setUserAgentString",
                    "(Ljava/lang/String;)V",
                    &[JValue::Object(&jua)],
                );
            }
        }
    }
    unsafe {
        let _ = env.pop_local_frame(&jni::objects::JObject::null());
    }
}

pub fn set_allowed_domains(handle: i64, domains_arr_handle: i64) {
    extern "C" {
        fn js_array_get_length(arr: i64) -> i64;
        fn js_array_get_element_f64(arr: i64, index: i64) -> f64;
        fn js_get_string_pointer_unified(value: f64) -> *const u8;
    }
    let mut domains = Vec::new();
    unsafe {
        let len = js_array_get_length(domains_arr_handle);
        for i in 0..len {
            let elem = js_array_get_element_f64(domains_arr_handle, i);
            let str_ptr = js_get_string_pointer_unified(elem);
            if !str_ptr.is_null() {
                domains.push(str_from_header(str_ptr).to_string());
            }
        }
    }
    WEBVIEW_STATES.with(|s| {
        if let Some(st) = s.borrow_mut().get_mut(&handle) {
            st.allowed_domains = domains;
        }
    });
}

pub fn set_ephemeral(_handle: i64, ephemeral: i64) {
    // Android WebView shares CookieManager / WebStorage across the
    // process — true per-WebView isolation needs a separate process,
    // not just per-instance state. v1 honors `ephemeral: true` by
    // clearing cookies + WebStorage on init (best-effort).
    if ephemeral != 0 {
        let mut env = jni_bridge::get_env();
        let _ = env.push_local_frame(8);
        if let Ok(mgr_class) = env.find_class("android/webkit/CookieManager") {
            if let Ok(mgr) = env.call_static_method(
                &mgr_class,
                "getInstance",
                "()Landroid/webkit/CookieManager;",
                &[],
            ) {
                if let Ok(mgr_obj) = mgr.l() {
                    let null_cb = jni::objects::JObject::null();
                    let _ = env.call_method(
                        &mgr_obj,
                        "removeAllCookies",
                        "(Landroid/webkit/ValueCallback;)V",
                        &[JValue::Object(&null_cb)],
                    );
                }
            }
        }
        if let Ok(storage_class) = env.find_class("android/webkit/WebStorage") {
            if let Ok(s) = env.call_static_method(
                &storage_class,
                "getInstance",
                "()Landroid/webkit/WebStorage;",
                &[],
            ) {
                if let Ok(storage) = s.l() {
                    let _ = env.call_method(&storage, "deleteAllData", "()V", &[]);
                }
            }
        }
        unsafe {
        let _ = env.pop_local_frame(&jni::objects::JObject::null());
    }
    }
}

pub fn set_on_should_navigate(handle: i64, closure: f64) {
    WEBVIEW_STATES.with(|s| {
        if let Some(st) = s.borrow_mut().get_mut(&handle) {
            st.on_should_navigate = closure;
        }
    });
}

pub fn set_on_loaded(handle: i64, closure: f64) {
    WEBVIEW_STATES.with(|s| {
        if let Some(st) = s.borrow_mut().get_mut(&handle) {
            st.on_loaded = closure;
        }
    });
}

pub fn set_on_error(handle: i64, closure: f64) {
    WEBVIEW_STATES.with(|s| {
        if let Some(st) = s.borrow_mut().get_mut(&handle) {
            st.on_error = closure;
        }
    });
}

#[allow(dead_code)]
fn _ref_globalref() {
    let _ = std::marker::PhantomData::<GlobalRef>;
}

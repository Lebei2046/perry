//! Promise implementation for async/await support
//!
//! This is a simplified Promise implementation for the Perry runtime.
//! It supports basic resolve/reject and then/catch chaining.

use std::cell::RefCell;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

/// Return true iff `value`'s NaN-box tag indicates it cannot possibly
/// be a Promise pointer or a thenable object — i.e. a number,
/// undefined, null, true, false, or a non-pointer-tagged f64. The
/// async-to-generator transform emits `Promise.resolve(x).then(...)`
/// per `await`; in the common case `x` is one of these primitives.
/// Skipping the `is_promise` + `assimilate_thenable` probes for them
/// removes ~600 ns of work from every await steady-state iteration.
#[inline(always)]
fn is_definitely_primitive(value: f64) -> bool {
    const POINTER_TAG: u64 = 0x7FFD_0000_0000_0000;
    const TAG_MASK: u64 = 0xFFFF_0000_0000_0000;
    let bits = value.to_bits();
    let tag = bits & TAG_MASK;
    // Only POINTER_TAG-tagged values can be promises or thenable
    // objects. Strings (STRING_TAG), bigints (BIGINT_TAG), int32s
    // (INT32_TAG), bool/null/undefined (TAG_xxx), and raw f64
    // numbers (no special tag) are all primitives in spec terms.
    tag != POINTER_TAG
}

// Instrumentation counters (set PERRY_MT_PROFILE=1 to print at exit).
pub static MT_RUN_COUNT: AtomicU64 = AtomicU64::new(0);
pub static MT_THENABLE_PROBE_COUNT: AtomicU64 = AtomicU64::new(0);
pub static MT_PROMISE_NEW_COUNT: AtomicU64 = AtomicU64::new(0);
pub static MT_PROMISE_THEN_COUNT: AtomicU64 = AtomicU64::new(0);
pub static MT_PROMISE_RESOLVED_COUNT: AtomicU64 = AtomicU64::new(0);
pub static MT_INNER_PROMISE_UNWRAP_COUNT: AtomicU64 = AtomicU64::new(0);
pub static MT_TIME_NS_QUEUE: AtomicU64 = AtomicU64::new(0);
pub static MT_TIME_NS_CALLBACK: AtomicU64 = AtomicU64::new(0);
pub static MT_TIME_NS_RESOLVE: AtomicU64 = AtomicU64::new(0);
pub static MT_FAST_PATH_HIT: AtomicU64 = AtomicU64::new(0);
pub static MT_FAST_PATH_MISS: AtomicU64 = AtomicU64::new(0);

extern "C" fn mt_profile_atexit() {
    if std::env::var_os("PERRY_MT_PROFILE").is_none() {
        return;
    }
    eprintln!(
        "[mt-profile] runs={} resolved={} then={} new={} unwrap={} thenable_probe={}",
        MT_RUN_COUNT.load(Ordering::Relaxed),
        MT_PROMISE_RESOLVED_COUNT.load(Ordering::Relaxed),
        MT_PROMISE_THEN_COUNT.load(Ordering::Relaxed),
        MT_PROMISE_NEW_COUNT.load(Ordering::Relaxed),
        MT_INNER_PROMISE_UNWRAP_COUNT.load(Ordering::Relaxed),
        MT_THENABLE_PROBE_COUNT.load(Ordering::Relaxed),
    );
    let q = MT_TIME_NS_QUEUE.load(Ordering::Relaxed);
    let cb = MT_TIME_NS_CALLBACK.load(Ordering::Relaxed);
    let rs = MT_TIME_NS_RESOLVE.load(Ordering::Relaxed);
    eprintln!(
        "[mt-profile] time(ms): queue={:.1} callback={:.1} resolve={:.1}",
        q as f64 / 1e6,
        cb as f64 / 1e6,
        rs as f64 / 1e6,
    );
    eprintln!(
        "[mt-profile] fast_path: hit={} miss={}",
        MT_FAST_PATH_HIT.load(Ordering::Relaxed),
        MT_FAST_PATH_MISS.load(Ordering::Relaxed),
    );
    eprintln!(
        "[mt-profile] closure: alloc={} cap_singleton_hit={} cap_singleton_miss={}",
        crate::closure::CLOSURE_ALLOC_COUNT.load(Ordering::Relaxed),
        crate::closure::CLOSURE_CAP_SINGLETON_HIT.load(Ordering::Relaxed),
        crate::closure::CLOSURE_CAP_SINGLETON_MISS.load(Ordering::Relaxed),
    );
}

static MT_PROFILE_REG: std::sync::Once = std::sync::Once::new();
fn mt_profile_register() {
    MT_PROFILE_REG.call_once(|| unsafe {
        extern "C" {
            fn atexit(cb: extern "C" fn()) -> i32;
        }
        atexit(mt_profile_atexit);
    });
}

// ─── Iter-result scratch slot (async-step driver fast path) ───────
//
// `await x` rewrites to a generator state machine whose `next()`
// closure used to allocate `{value, done}` per call (one alloc per
// await, plus two PropertyGet linear scans by the async-step driver
// that immediately consumes both fields). On a 200k-await benchmark
// that's 200k object allocs purely as a 2-field carrier between
// generator and step driver.
//
// We replace the alloc with a thread-local pair: the state machine
// writes (value, done) via `js_iter_result_set` and returns `undefined`;
// the step driver reads them back via `js_iter_result_get_value` /
// `js_iter_result_get_done`. The slot is overwritten on every
// `next()` call — safe because the async-step driver synchronously
// consumes both fields immediately after the call returns, with no
// intervening generator activity.
//
// The transform only emits these helpers for `was_plain_async`
// generators (async functions rewritten via async→generator). User-
// visible generators (`function*`) still allocate real `{value, done}`
// objects so `for...of` and external consumers see the spec shape.
thread_local! {
    static ITER_RESULT_VALUE: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
    static ITER_RESULT_DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub static MT_ITER_RESULT_SET_COUNT: AtomicU64 = AtomicU64::new(0);

/// Write the iter-result scratch slot. Returns `undefined` so callers
/// can `return js_iter_result_set(v, d)` from the generator's `next()`
/// state-machine without a separate trailing `return undefined`.
#[no_mangle]
pub extern "C" fn js_iter_result_set(value: f64, done: i32) -> f64 {
    MT_ITER_RESULT_SET_COUNT.fetch_add(1, Ordering::Relaxed);
    ITER_RESULT_VALUE.with(|c| c.set(value));
    ITER_RESULT_DONE.with(|c| c.set(done != 0));
    f64::from_bits(crate::value::TAG_UNDEFINED)
}

/// Read the value half of the iter-result scratch slot.
#[no_mangle]
pub extern "C" fn js_iter_result_get_value() -> f64 {
    ITER_RESULT_VALUE.with(|c| c.get())
}

/// Read the done half as a NaN-boxed bool (TAG_TRUE / TAG_FALSE) so it
/// can flow into any control-flow / property context without a
/// separate conversion.
#[no_mangle]
pub extern "C" fn js_iter_result_get_done() -> f64 {
    const TAG_TRUE: u64 = 0x7FFC_0000_0000_0004;
    const TAG_FALSE: u64 = 0x7FFC_0000_0000_0003;
    let d = ITER_RESULT_DONE.with(|c| c.get());
    f64::from_bits(if d { TAG_TRUE } else { TAG_FALSE })
}

/// GC root scanner. The value half can hold pointer NaN-box values
/// (Promise pointers, object pointers from awaited values); register
/// this with the GC so they survive a collection that lands between
/// `js_iter_result_set` and the next `js_iter_result_get_value`.
pub fn scan_iter_result_root(mark: &mut dyn FnMut(f64)) {
    let v = ITER_RESULT_VALUE.with(|c| c.get());
    mark(v);
}

/// Promise state
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromiseState {
    Pending = 0,
    Fulfilled = 1,
    Rejected = 2,
}

/// Closure pointer type for promise handlers (closures, not raw function pointers)
pub type ClosurePtr = *const crate::closure::ClosureHeader;

/// A Promise represents an eventual completion (or failure) of an async operation
#[repr(C)]
pub struct Promise {
    /// Current state of the promise
    pub(crate) state: PromiseState,
    /// The resolved value (if fulfilled)
    pub(crate) value: f64,
    /// The rejection reason (if rejected)
    pub(crate) reason: f64,
    /// Closure to run when fulfilled (null if none)
    pub(crate) on_fulfilled: ClosurePtr,
    /// Closure to run when rejected (null if none)
    pub(crate) on_rejected: ClosurePtr,
    /// Next promise in the chain (for .then())
    pub(crate) next: *mut Promise,
}

impl Promise {
    fn new() -> Self {
        Promise {
            state: PromiseState::Pending,
            value: 0.0,
            reason: 0.0,
            on_fulfilled: ptr::null(),
            on_rejected: ptr::null(),
            next: ptr::null_mut(),
        }
    }
}

/// One entry in the microtask queue. Two shapes:
///
/// `Promise(p, value, is_fulfilled)` — the legacy shape: we'll dispatch
/// to `(*p).on_fulfilled` or `on_rejected` depending on the bool, then
/// resolve `(*p).next` with the callback's return value.
///
/// `Inline(cb, value, next, is_fulfilled)` — the fast-path shape used
/// by `js_promise_resolved_then` when the awaited value is a primitive.
/// We've skipped allocating a source promise; the callback is carried
/// inline. Dispatch is identical from here: invoke `cb(value)` (or
/// `cb_rej(value)` — only one is non-null per entry), propagate the
/// result to `next`. Saves one Promise allocation per `await` of a
/// primitive value, which is the steady-state pattern for the async-to-
/// generator transform.
#[derive(Clone, Copy)]
enum Task {
    Promise(*mut Promise, f64, bool),
    Inline(ClosurePtr, f64, *mut Promise, bool),
    /// Direct dispatch to a 2-arg async-step closure. Equivalent to
    /// `Inline(then_v_arrow, value, next, true)` where `then_v_arrow`
    /// is a wrapper that calls `step(value, is_error)` — but skips the
    /// then_v_arrow alloc + dispatch by carrying `step_closure` and
    /// the `is_error` flag directly. Saves one closure allocation
    /// per await on the steady-state primitive-await path.
    AsyncStep(ClosurePtr, f64, *mut Promise, bool),
}

// Global task queue for pending promise callbacks. Must be FIFO per
// ECMAScript microtask semantics: `Promise.resolve(1).then(...)` and
// `Promise.resolve(2).then(...)` registered in source order must run
// their continuations in source order (1 first, then 2). Using a
// `Vec` with `.pop()` produces LIFO ordering, breaking every test
// that prints inside multiple parallel promise chains.
thread_local! {
    static TASK_QUEUE: RefCell<std::collections::VecDeque<Task>>
        = const { RefCell::new(std::collections::VecDeque::new()) };
}

/// Returns 1 iff the current thread's microtask queue has at least one
/// pending entry, 0 otherwise. Used by the codegen-emitted event-loop
/// active-handle check (#591) so the loop doesn't exit while a chained
/// `.then(...)` callback is waiting in the queue. Without this, an
/// `await` driven by perry-stdlib's `js_stdlib_process_pending`
/// resolution path could push a continuation to TASK_QUEUE in the
/// SAME body iteration that flips the active-handle counter to zero;
/// the next header check would then exit before the next body's
/// microtask drain runs.
#[no_mangle]
pub extern "C" fn js_microtasks_pending() -> i32 {
    TASK_QUEUE.with(|q| if q.borrow().is_empty() { 0 } else { 1 })
}

/// Allocate a new Promise
#[no_mangle]
pub extern "C" fn js_promise_new() -> *mut Promise {
    MT_PROMISE_NEW_COUNT.fetch_add(1, Ordering::Relaxed);
    let raw = crate::gc::gc_malloc(std::mem::size_of::<Promise>(), crate::gc::GC_TYPE_PROMISE);
    let promise = raw as *mut Promise;
    unsafe {
        ptr::write(promise, Promise::new());
    }
    promise
}

/// Free a Promise (no-op — GC handles deallocation)
#[no_mangle]
pub extern "C" fn js_promise_free(_promise: *mut Promise) {
    // GC handles deallocation now
}

/// Get promise state (0=pending, 1=fulfilled, 2=rejected)
#[no_mangle]
pub extern "C" fn js_promise_state(promise: *mut Promise) -> i32 {
    if promise.is_null() {
        return -1;
    }
    unsafe { (*promise).state as i32 }
}

/// Get promise value (if fulfilled)
#[no_mangle]
pub extern "C" fn js_promise_value(promise: *mut Promise) -> f64 {
    if promise.is_null() {
        return 0.0;
    }

    unsafe { (*promise).value }
}

/// Get promise reason (if rejected)
#[no_mangle]
pub extern "C" fn js_promise_reason(promise: *mut Promise) -> f64 {
    if promise.is_null() {
        return 0.0;
    }
    unsafe { (*promise).reason }
}

/// Get promise result (value if fulfilled, reason if rejected)
/// This is what await should use to get the result of a promise.
/// For fulfilled promises, returns the resolved value.
/// For rejected promises, returns the rejection reason.
/// For pending promises (should not happen in normal use), returns 0.0.
#[no_mangle]
pub extern "C" fn js_promise_result(promise: *mut Promise) -> f64 {
    if promise.is_null() {
        return 0.0;
    }
    unsafe {
        match (*promise).state {
            PromiseState::Fulfilled => (*promise).value,
            PromiseState::Rejected => (*promise).reason,
            PromiseState::Pending => 0.0,
        }
    }
}

/// Resolve a promise with a value
#[no_mangle]
pub extern "C" fn js_promise_resolve(promise: *mut Promise, value: f64) {
    if promise.is_null() {
        return;
    }
    unsafe {
        if (*promise).state != PromiseState::Pending {
            return; // Already settled
        }
        (*promise).state = PromiseState::Fulfilled;
        (*promise).value = value;

        // Schedule callbacks. Push to TASK_QUEUE whenever there's anything
        // for the microtask runner to do — either invoke the user callback,
        // or propagate the value to the chained `next` promise. Issue #236:
        // pre-fix the queue push only fired when `on_fulfilled` was non-null,
        // so `.then(console.log)` (where `console.log`-as-value lowers to
        // a NULL ClosurePtr sentinel — see expr.rs:GlobalGet→PropertyGet
        // value path) skipped the queue entirely; the chained promise then
        // never settled and `await chained` busy-waited forever.
        if !(*promise).on_fulfilled.is_null() || !(*promise).next.is_null() {
            TASK_QUEUE.with(|q| {
                q.borrow_mut().push_back(Task::Promise(promise, value, true));
            });
        }
    }
    // Issue #84: an `await` busy-wait that called `js_timer_tick` (or any
    // tick fn) which then resolved this promise needs to skip the
    // following `js_wait_for_event` sleep — otherwise it blocks for the
    // 1 s idle cap before the loop re-checks promise state. The notify
    // sets the flag so the immediately-following wait returns at once.
    crate::event_pump::js_notify_main_thread();
}

/// Resolve a promise with another promise (Promise chaining/unwrapping)
/// When the inner promise resolves, the outer promise adopts its value
#[no_mangle]
pub extern "C" fn js_promise_resolve_with_promise(outer: *mut Promise, inner: *mut Promise) {
    if outer.is_null() || inner.is_null() {
        return;
    }

    unsafe {
        if (*outer).state != PromiseState::Pending {
            return; // Already settled
        }

        // Check inner promise state
        match (*inner).state {
            PromiseState::Fulfilled => {
                // Inner already resolved - resolve outer with inner's value
                js_promise_resolve(outer, (*inner).value);
            }
            PromiseState::Rejected => {
                // Inner already rejected - reject outer with inner's reason
                js_promise_reject(outer, (*inner).reason);
            }
            PromiseState::Pending => {
                // Inner is pending.
                //
                // Perf fast path: if inner has no callbacks AND no
                // chained `next` already, we can simply chain outer
                // as inner's next. When inner settles, the microtask
                // runner's "callback null but next non-null" arm at
                // `js_promise_run_microtasks` will propagate the
                // value/reason to outer directly — same observable
                // semantics as forward_resolve/forward_reject but
                // skips two closure allocations AND a microtask hop.
                //
                // This is the steady-state shape inside the async-
                // step driver: each await's `step()` returns a fresh
                // promise from `Promise.resolve(v).then(...)` whose
                // `next` is null and whose callbacks were just set
                // on the inner source — the returned outer wrapper
                // itself is callback-less. Eliminating that hop is
                // the largest single win in the per-await steady
                // state.
                if (*inner).on_fulfilled.is_null()
                    && (*inner).on_rejected.is_null()
                    && (*inner).next.is_null()
                {
                    (*inner).next = outer;
                    return;
                }

                // Slow path: inner already has callbacks or a chained
                // `next`. Fall back to the forwarding-closure shape
                // so we don't clobber existing wiring.
                let outer_i64 = outer as i64;

                // Create a resolve forwarding closure
                let resolve_closure =
                    crate::closure::js_closure_alloc(promise_forward_resolve as *const u8, 1);
                crate::closure::js_closure_set_capture_ptr(resolve_closure, 0, outer_i64);

                // Create a reject forwarding closure
                let reject_closure =
                    crate::closure::js_closure_alloc(promise_forward_reject as *const u8, 1);
                crate::closure::js_closure_set_capture_ptr(reject_closure, 0, outer_i64);

                // Register the forwarding callbacks on the inner promise
                (*inner).on_fulfilled = resolve_closure;
                (*inner).on_rejected = reject_closure;
                (*inner).next = ptr::null_mut(); // Don't chain, we handle resolution ourselves
            }
        }
    }
}

/// Internal callback for forwarding resolve from inner to outer promise
extern "C" fn promise_forward_resolve(
    closure: *const crate::closure::ClosureHeader,
    value: f64,
) -> f64 {
    let outer_ptr = crate::closure::js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    js_promise_resolve(outer_ptr, value);
    0.0
}

/// Internal callback for forwarding reject from inner to outer promise
extern "C" fn promise_forward_reject(
    closure: *const crate::closure::ClosureHeader,
    reason: f64,
) -> f64 {
    let outer_ptr = crate::closure::js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    js_promise_reject(outer_ptr, reason);
    0.0
}

/// Reject a promise with a reason
#[no_mangle]
pub extern "C" fn js_promise_reject(promise: *mut Promise, reason: f64) {
    if promise.is_null() {
        return;
    }
    unsafe {
        if (*promise).state != PromiseState::Pending {
            return; // Already settled
        }
        (*promise).state = PromiseState::Rejected;
        (*promise).reason = reason;

        // Schedule callbacks. Same propagation rule as `js_promise_resolve`
        // (#236): push to the queue whenever there's a callback to invoke
        // OR a chained `next` promise to forward to.
        if !(*promise).on_rejected.is_null() || !(*promise).next.is_null() {
            TASK_QUEUE.with(|q| {
                q.borrow_mut().push_back(Task::Promise(promise, reason, false));
            });
        }
    }
    // Issue #84: see js_promise_resolve — same wake reasoning.
    crate::event_pump::js_notify_main_thread();
}

/// Register fulfillment callback, returns a new promise for chaining
#[no_mangle]
pub extern "C" fn js_promise_then(
    promise: *mut Promise,
    on_fulfilled: ClosurePtr,
    on_rejected: ClosurePtr,
) -> *mut Promise {
    MT_PROMISE_THEN_COUNT.fetch_add(1, Ordering::Relaxed);
    if promise.is_null() {
        return ptr::null_mut();
    }

    let next = js_promise_new();

    unsafe {
        (*promise).on_fulfilled = on_fulfilled;
        (*promise).on_rejected = on_rejected;
        (*promise).next = next;

        // If already settled, schedule callback immediately. Same propagation
        // rule as `js_promise_resolve`/`js_promise_reject` (#236): push to the
        // queue when there's either a callback to invoke OR a chained `next`
        // promise to forward to. `next` is always non-null here (we just
        // created it), so this is effectively unconditional — the explicit
        // checks document the intent.
        match (*promise).state {
            PromiseState::Fulfilled => {
                if !on_fulfilled.is_null() || !next.is_null() {
                    TASK_QUEUE.with(|q| {
                        q.borrow_mut()
                            .push_back(Task::Promise(promise, (*promise).value, true));
                    });
                }
            }
            PromiseState::Rejected => {
                if !on_rejected.is_null() || !next.is_null() {
                    TASK_QUEUE.with(|q| {
                        q.borrow_mut()
                            .push_back(Task::Promise(promise, (*promise).reason, false));
                    });
                }
            }
            PromiseState::Pending => {}
        }
    }

    next
}

/// Like `js_promise_then` but skips the allocation of a chained `next`
/// promise. Used by callers that only need the side effect of running
/// the handler (Promise.all, Promise.race, internal forwarders), not
/// the chained promise. Saves one Promise alloc per call — material on
/// Promise.all of N inputs which today allocates N never-used `next`
/// promises.
pub(crate) fn js_promise_attach_handlers(
    promise: *mut Promise,
    on_fulfilled: ClosurePtr,
    on_rejected: ClosurePtr,
) {
    if promise.is_null() {
        return;
    }
    unsafe {
        (*promise).on_fulfilled = on_fulfilled;
        (*promise).on_rejected = on_rejected;
        // No next — caller doesn't want a chained promise.

        match (*promise).state {
            PromiseState::Fulfilled => {
                if !on_fulfilled.is_null() {
                    TASK_QUEUE.with(|q| {
                        q.borrow_mut()
                            .push_back(Task::Promise(promise, (*promise).value, true));
                    });
                }
            }
            PromiseState::Rejected => {
                if !on_rejected.is_null() {
                    TASK_QUEUE.with(|q| {
                        q.borrow_mut()
                            .push_back(Task::Promise(promise, (*promise).reason, false));
                    });
                }
            }
            PromiseState::Pending => {}
        }
    }
}

/// Register rejection callback, returns a new promise for chaining
/// This is equivalent to .catch(onRejected) in JavaScript
#[no_mangle]
pub extern "C" fn js_promise_catch(promise: *mut Promise, on_rejected: ClosurePtr) -> *mut Promise {
    js_promise_then(promise, ptr::null(), on_rejected)
}

/// Register finally callback, returns a new promise for chaining.
/// This is equivalent to .finally(onFinally) in JavaScript.
///
/// Per spec, `.finally(cb)` must:
///   - Call `cb()` (ignoring its return value)
///   - Propagate the upstream fulfilled VALUE (not cb's return) to `next`
///   - Re-reject with the upstream rejection REASON if the upstream rejected
///
/// The spec (and Node.js) requires `.finally(cb)` to take ONE more microtask
/// tick than a plain `.then(cb)`.  We achieve this by setting `promise.next =
/// null` so the microtask runner does NOT resolve `next` after invoking the
/// wrapper callback — the wrappers resolve `next` themselves, via an extra
/// `js_promise_then(resolved_promise, passthrough)` hop that adds one queue
/// entry before `next` settles.
///
/// Capture layout for each wrapper: [on_finally, next_promise_ptr]
/// Capture layout for passthrough closures: [next_promise_ptr, value_or_reason]
#[no_mangle]
pub extern "C" fn js_promise_finally(
    promise: *mut Promise,
    on_finally: ClosurePtr,
) -> *mut Promise {
    use crate::closure::{js_closure_alloc, js_closure_set_capture_ptr};

    // Create the `next` promise that callers chain off.
    let next = js_promise_new();
    let next_i64 = next as i64;

    // Build the fulfilled wrapper: captures [on_finally, next].
    let fulfill_wrap = js_closure_alloc(finally_fulfill_wrapper as *const u8, 2);
    js_closure_set_capture_ptr(fulfill_wrap, 0, on_finally as i64);
    js_closure_set_capture_ptr(fulfill_wrap, 1, next_i64);

    // Build the rejected wrapper: captures [on_finally, next].
    let reject_wrap = js_closure_alloc(finally_reject_wrapper as *const u8, 2);
    js_closure_set_capture_ptr(reject_wrap, 0, on_finally as i64);
    js_closure_set_capture_ptr(reject_wrap, 1, next_i64);

    // Register wrappers on `promise`.  Crucially, set `promise.next = null`
    // so the microtask runner does NOT attempt to resolve `next` after calling
    // the wrapper — each wrapper handles `next` settlement itself via the
    // extra-tick passthrough pattern.
    unsafe {
        (*promise).on_fulfilled = fulfill_wrap;
        (*promise).on_rejected = reject_wrap;
        (*promise).next = ptr::null_mut(); // wrappers own next; runner must not touch it

        // If the promise is already settled, push its task now.
        match (*promise).state {
            PromiseState::Fulfilled => {
                TASK_QUEUE.with(|q| {
                    q.borrow_mut()
                        .push_back(Task::Promise(promise, (*promise).value, true));
                });
            }
            PromiseState::Rejected => {
                TASK_QUEUE.with(|q| {
                    q.borrow_mut()
                        .push_back(Task::Promise(promise, (*promise).reason, false));
                });
            }
            PromiseState::Pending => {}
        }
    }

    next
}

/// Fulfilled-path wrapper for `.finally()`.
/// Captures [on_finally, next_promise].
/// Called with the upstream fulfilled `value`.
/// Runs `on_finally()`, then resolves `next` with `value` via ONE extra
/// microtask hop (matching Node.js `.finally()` microtask depth).
extern "C" fn finally_fulfill_wrapper(
    closure: *const crate::closure::ClosureHeader,
    value: f64,
) -> f64 {
    use crate::closure::{
        js_closure_alloc, js_closure_get_capture_ptr, js_closure_set_capture_ptr,
    };

    let on_finally = js_closure_get_capture_ptr(closure, 0) as *const crate::closure::ClosureHeader;
    let next = js_closure_get_capture_ptr(closure, 1) as *mut Promise;

    // Call the user's finally callback (ignoring its return value).
    if !on_finally.is_null() {
        let undef = f64::from_bits(crate::value::TAG_UNDEFINED);
        unsafe {
            crate::closure::js_closure_call1(on_finally, undef);
        }
    }

    // Add one extra microtask tick before settling `next` by registering a
    // passthrough closure on an already-resolved promise.  The runner will
    // enqueue it, call it next iteration, and THEN `next` gets resolved.
    if !next.is_null() {
        let pass = js_closure_alloc(finally_passthrough_fulfill as *const u8, 2);
        js_closure_set_capture_ptr(pass, 0, next as i64);
        crate::closure::js_closure_set_capture_f64(pass, 1, value);

        let undef_promise = js_promise_resolved(f64::from_bits(crate::value::TAG_UNDEFINED));
        // js_promise_then returns a new (discarded) promise; the side-effect
        // is enqueuing `pass` to run in the next microtask iteration.
        js_promise_then(undef_promise, pass, ptr::null());
    }

    // Return undefined.  Since promise.next is null (set in js_promise_finally),
    // the runner will not try to resolve anything with this return value.
    f64::from_bits(crate::value::TAG_UNDEFINED)
}

/// Passthrough closure for the extra hop in finally_fulfill_wrapper.
/// Captures [next_promise_ptr (i64), value (f64)].
/// Resolves `next` with `value`.
extern "C" fn finally_passthrough_fulfill(
    closure: *const crate::closure::ClosureHeader,
    _: f64,
) -> f64 {
    use crate::closure::{js_closure_get_capture_f64, js_closure_get_capture_ptr};
    let next = js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    let value = js_closure_get_capture_f64(closure, 1);
    if !next.is_null() {
        js_promise_resolve(next, value);
    }
    f64::from_bits(crate::value::TAG_UNDEFINED)
}

/// Rejected-path wrapper for `.finally()`.
/// Captures [on_finally, next_promise].
/// Called with the upstream rejection `reason`.
/// Runs `on_finally()`, then rejects `next` with `reason` via ONE extra
/// microtask hop.
extern "C" fn finally_reject_wrapper(
    closure: *const crate::closure::ClosureHeader,
    reason: f64,
) -> f64 {
    use crate::closure::{
        js_closure_alloc, js_closure_get_capture_ptr, js_closure_set_capture_ptr,
    };

    let on_finally = js_closure_get_capture_ptr(closure, 0) as *const crate::closure::ClosureHeader;
    let next = js_closure_get_capture_ptr(closure, 1) as *mut Promise;

    // Call the user's finally callback (ignoring its return value).
    if !on_finally.is_null() {
        let undef = f64::from_bits(crate::value::TAG_UNDEFINED);
        unsafe {
            crate::closure::js_closure_call1(on_finally, undef);
        }
    }

    // Add one extra microtask tick before rejecting `next`.
    if !next.is_null() {
        let pass = js_closure_alloc(finally_passthrough_reject as *const u8, 2);
        js_closure_set_capture_ptr(pass, 0, next as i64);
        crate::closure::js_closure_set_capture_f64(pass, 1, reason);

        let undef_promise = js_promise_resolved(f64::from_bits(crate::value::TAG_UNDEFINED));
        js_promise_then(undef_promise, pass, ptr::null());
    }

    f64::from_bits(crate::value::TAG_UNDEFINED)
}

/// Passthrough closure for the extra hop in finally_reject_wrapper.
/// Captures [next_promise_ptr (i64), reason (f64)].
/// Rejects `next` with `reason`.
extern "C" fn finally_passthrough_reject(
    closure: *const crate::closure::ClosureHeader,
    _: f64,
) -> f64 {
    use crate::closure::{js_closure_get_capture_f64, js_closure_get_capture_ptr};
    let next = js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    let reason = js_closure_get_capture_f64(closure, 1);
    if !next.is_null() {
        js_promise_reject(next, reason);
    }
    f64::from_bits(crate::value::TAG_UNDEFINED)
}

/// Process all pending promise callbacks (run microtasks)
#[no_mangle]
pub extern "C" fn js_promise_run_microtasks() -> i32 {
    mt_profile_register();
    let mut ran = 0;

    // First, tick timers to resolve any expired timer promises
    ran += crate::timer::js_timer_tick();

    // Process callback timers (setTimeout with callbacks)
    ran += crate::timer::js_callback_timer_tick();

    // Process interval timers (setInterval)
    ran += crate::timer::js_interval_timer_tick();

    // Process any scheduled resolutions (simulates async completions)
    ran += process_scheduled_resolves();

    // Process pending thread results (from perry/thread spawn)
    ran += crate::thread::js_thread_process_pending();

    // Drain queued microtasks (from queueMicrotask() calls).
    crate::builtins::js_drain_queued_microtasks();

    // Then process the task queue.
    //
    // ── Exception trap (Issue #...): install ONE setjmp for the WHOLE
    // loop body, instead of a fresh setjmp per microtask. The previous
    // shape paid setjmp+js_try_push/end every microtask just so that a
    // `throw` from a callback could be re-routed to reject the chained
    // `next` promise. setjmp+longjmp on aarch64 saves ~16 callee-saved
    // x-regs and ~8 d-regs per call — that's ~25 ns per microtask, and
    // an async benchmark with 200k microtasks pays ~5 ms in setjmp cost
    // alone. The single outer setjmp captures the same "throw out of a
    // microtask body" case (since `js_throw` longjmps to the most recent
    // try block; if no user try is in scope, this one is it). When the
    // longjmp lands, we read the current promise context out of a
    // thread-local set just before invoking the callback, reject its
    // `next`, and continue the loop.
    extern "C" {
        fn setjmp(env: *mut i32) -> i32;
    }

    // Install the trap. We set up CURRENT_MICROTASK_PROMISE (or
    // INLINE_TRAP_NEXT for inline-callback tasks) before the callback
    // so the rejection path knows which `next` to reject.
    thread_local! {
        static CURRENT_MICROTASK_PROMISE: std::cell::Cell<*mut Promise>
            = const { std::cell::Cell::new(std::ptr::null_mut()) };
        static INLINE_TRAP_NEXT: std::cell::Cell<*mut Promise>
            = const { std::cell::Cell::new(std::ptr::null_mut()) };
    }

    let trap_buf = crate::exception::js_try_push();
    // SAFETY: The setjmp call must remain in this stack frame; we
    // longjmp to it from `js_throw` only while this frame is still
    // alive (inside the loop below).
    let jumped = unsafe { setjmp(trap_buf) };
    if jumped != 0 {
        // A microtask's callback threw and unwound here. Read the
        // exception, clear it, and reject the `next` promise of the
        // microtask that was running. js_try_end is intentionally NOT
        // called yet — we want the trap to remain in scope for the
        // rest of the loop.
        let exc = crate::exception::js_get_exception();
        crate::exception::js_clear_exception();
        let cur = CURRENT_MICROTASK_PROMISE.with(|c| c.replace(std::ptr::null_mut()));
        if !cur.is_null() {
            unsafe {
                if !(*cur).next.is_null() {
                    js_promise_reject((*cur).next, exc);
                }
            }
            ran += 1;
        } else {
            let inline_next = INLINE_TRAP_NEXT.with(|c| c.replace(std::ptr::null_mut()));
            if !inline_next.is_null() {
                js_promise_reject(inline_next, exc);
                ran += 1;
            }
        }
    }

    let prof = std::env::var_os("PERRY_MT_PROFILE").is_some();
    loop {
        let t0 = if prof { Some(std::time::Instant::now()) } else { None };
        let task = TASK_QUEUE.with(|q| q.borrow_mut().pop_front());
        if let Some(t) = t0 {
            MT_TIME_NS_QUEUE.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }

        match task {
            None => break,
            Some(Task::Promise(promise, value, is_fulfilled)) => {
                MT_RUN_COUNT.fetch_add(1, Ordering::Relaxed);
                unsafe {
                    let callback = if is_fulfilled {
                        (*promise).on_fulfilled
                    } else {
                        (*promise).on_rejected
                    };

                    // No callback registered → propagate the value/reason
                    // to the next promise without invoking anything.
                    if callback.is_null() {
                        if !(*promise).next.is_null() {
                            if is_fulfilled {
                                js_promise_resolve((*promise).next, value);
                            } else {
                                js_promise_reject((*promise).next, value);
                            }
                        }
                        ran += 1;
                        continue;
                    }

                    // Record the running promise so the trap (above)
                    // can reject its `next` if the callback throws.
                    CURRENT_MICROTASK_PROMISE.with(|c| c.set(promise));

                    let t1 = if prof { Some(std::time::Instant::now()) } else { None };
                    let result = crate::closure::js_closure_call1(callback, value);
                    if let Some(t) = t1 {
                        MT_TIME_NS_CALLBACK
                            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    }

                    // Callback returned normally; clear the running
                    // marker so a stray longjmp from a later (nested)
                    // microtask doesn't misattribute its rejection.
                    CURRENT_MICROTASK_PROMISE.with(|c| c.set(std::ptr::null_mut()));

                    let t2 = if prof { Some(std::time::Instant::now()) } else { None };
                    if !(*promise).next.is_null() {
                        propagate_callback_result(result, (*promise).next);
                    }
                    if let Some(t) = t2 {
                        MT_TIME_NS_RESOLVE
                            .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    }
                }
                ran += 1;
            }
            Some(Task::Inline(callback, value, next, is_fulfilled)) => {
                MT_RUN_COUNT.fetch_add(1, Ordering::Relaxed);
                // Inline tasks are produced by `js_promise_resolved_then`
                // (the `Promise.resolve(<primitive>).then(cb_f, cb_e)`
                // fast path). We've already skipped allocating the
                // source promise — now dispatch directly: invoke the
                // stored callback, propagate the result to `next`.
                if callback.is_null() {
                    if !next.is_null() {
                        if is_fulfilled {
                            js_promise_resolve(next, value);
                        } else {
                            js_promise_reject(next, value);
                        }
                    }
                    ran += 1;
                    continue;
                }

                // For exception unwinding, mirror the Promise variant:
                // store a fake `cur` whose `.next` is what we want to
                // reject if the callback throws. Allocate a minimal
                // stub on the GC heap so the trap path still finds a
                // valid `*mut Promise`. This is rarely hit (only on
                // user-throw inside the inline callback) and we can
                // afford the alloc on the slow path.
                INLINE_TRAP_NEXT.with(|c| c.set(next));

                let t1 = if prof { Some(std::time::Instant::now()) } else { None };
                let result = unsafe { crate::closure::js_closure_call1(callback, value) };
                if let Some(t) = t1 {
                    MT_TIME_NS_CALLBACK
                        .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                }

                INLINE_TRAP_NEXT.with(|c| c.set(std::ptr::null_mut()));

                let t2 = if prof { Some(std::time::Instant::now()) } else { None };
                if !next.is_null() {
                    propagate_callback_result(result, next);
                }
                if let Some(t) = t2 {
                    MT_TIME_NS_RESOLVE
                        .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                }
                ran += 1;
            }
            Some(Task::AsyncStep(step_closure, value, next, is_error)) => {
                MT_RUN_COUNT.fetch_add(1, Ordering::Relaxed);
                // Direct dispatch of the async-step closure. Skips the
                // then_v_arrow / then_e_arrow wrapper that would
                // otherwise be invoked as the on_fulfilled / on_rejected
                // callback — the wrapper just calls
                // `__step(value, is_error)` which is exactly what we do
                // here with two fewer indirections (closure alloc +
                // closure call).
                if step_closure.is_null() {
                    if !next.is_null() {
                        if is_error {
                            js_promise_reject(next, value);
                        } else {
                            js_promise_resolve(next, value);
                        }
                    }
                    ran += 1;
                    continue;
                }
                INLINE_TRAP_NEXT.with(|c| c.set(next));

                let t1 = if prof { Some(std::time::Instant::now()) } else { None };
                let is_error_bits = if is_error {
                    f64::from_bits(0x7FFC_0000_0000_0004) // TAG_TRUE
                } else {
                    f64::from_bits(0x7FFC_0000_0000_0003) // TAG_FALSE
                };
                let result = unsafe {
                    crate::closure::js_closure_call2(step_closure, value, is_error_bits)
                };
                if let Some(t) = t1 {
                    MT_TIME_NS_CALLBACK
                        .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                }

                INLINE_TRAP_NEXT.with(|c| c.set(std::ptr::null_mut()));

                let t2 = if prof { Some(std::time::Instant::now()) } else { None };
                if !next.is_null() {
                    propagate_callback_result(result, next);
                }
                if let Some(t) = t2 {
                    MT_TIME_NS_RESOLVE
                        .fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                }
                ran += 1;
            }
        }
    }

    crate::exception::js_try_end();

    ran
}

/// Common tail of a microtask: take the value the callback returned
/// and feed it into `next`. If the callback returned a Promise, the
/// chained promise must ADOPT that promise's eventual state per
/// ECMAScript spec (Issue #256) — store-and-resolve breaks deep
/// generator-state-machine chains.
#[inline]
fn propagate_callback_result(result: f64, next: *mut Promise) {
    unsafe {
        if js_value_is_promise(result) != 0 {
            let inner = crate::value::js_nanbox_get_pointer(result) as *mut Promise;
            if !inner.is_null() && inner != next {
                js_promise_resolve_with_promise(next, inner);
            } else {
                js_promise_resolve(next, result);
            }
        } else {
            js_promise_resolve(next, result);
        }
    }
}

/// Create a resolved promise with the given value.
///
/// Matches ES spec `Promise.resolve(x)`: when `x` is itself a Promise the
/// returned promise adopts its state instead of storing the inner Promise
/// pointer as a plain value. This is the path async-function `return <expr>`
/// lowers through (see `perry-codegen/src/stmt.rs::Stmt::Return`) — without
/// the unwrap, `async function produce(): Promise<T> { return new Promise(...) }`
/// would return a promise whose `value` is a NaN-boxed pointer to the inner
/// Promise struct, so `await produce()` would see `typeof = 'object'` with all
/// user fields undefined (the Promise struct's layout) before the inner's
/// `setTimeout`/`resolve` ever fires. Closes #77.
#[no_mangle]
pub extern "C" fn js_promise_resolved(value: f64) -> *mut Promise {
    MT_PROMISE_RESOLVED_COUNT.fetch_add(1, Ordering::Relaxed);
    // FAST PATH: NaN-boxed primitives (numbers, undefined, null, bool,
    // raw f64s) are not pointers to thenables/promises. We can build a
    // pre-fulfilled promise and skip the `is_promise` + `assimilate`
    // probes — both are slow in the steady state of the async-to-
    // generator pattern (`Promise.resolve(<primitive>).then(...)` per
    // await). The probes still run on real pointers below.
    if is_definitely_primitive(value) {
        let promise = js_promise_new();
        unsafe {
            (*promise).state = PromiseState::Fulfilled;
            (*promise).value = value;
        }
        return promise;
    }
    let promise = js_promise_new();
    if js_value_is_promise(value) != 0 {
        MT_INNER_PROMISE_UNWRAP_COUNT.fetch_add(1, Ordering::Relaxed);
        let inner = crate::value::js_nanbox_get_pointer(value) as *mut Promise;
        if !inner.is_null() && inner != promise {
            js_promise_resolve_with_promise(promise, inner);
            return promise;
        }
    }

    // Issue #586: ECMAScript thenable assimilation. The async-to-generator
    // transform rewrites every `await x` into `Promise.resolve(x).then(...)`
    // — which means thenable assimilation has to happen here, not in the
    // codegen-side `Expr::Await` lowering. `js_assimilate_thenable` returns
    // a fresh Promise wrapper that follows the thenable's `.then(resolve,
    // reject)` callbacks; chain its eventual state into our outer promise
    // via the same `js_promise_resolve_with_promise` pattern as the real-
    // Promise arm above. Drizzle's `QueryPromise` (`then` triggers the SQL
    // round-trip) is the load-bearing motivating case (#488).
    let assim = js_assimilate_thenable(value);
    if assim.to_bits() != value.to_bits() && js_value_is_promise(assim) != 0 {
        let inner = crate::value::js_nanbox_get_pointer(assim) as *mut Promise;
        if !inner.is_null() && inner != promise {
            js_promise_resolve_with_promise(promise, inner);
            return promise;
        }
    }

    js_promise_resolve(promise, value);
    promise
}

/// Fused fast path for `Promise.resolve(value).then(cb_f, cb_e)` —
/// the steady-state shape of the async-to-generator transform's
/// per-`await` lowering. The naive sequence is:
///
///   p1 = js_promise_resolved(value)              // alloc Promise #1
///   p2 = js_promise_then(p1, cb_f, cb_e)         // alloc Promise #2
///   return p2
///
/// Per-await we pay 2 Promise allocations + 2 TASK_QUEUE round-trips
/// (push to queue, pop, dispatch cb_f). For a 200k-await benchmark
/// that's 400k allocations — the dominant per-microtask cost.
///
/// The fast path:
///   if `value` is a primitive (no possibility of being a thenable
///   or another Promise), allocate ONE promise (`next`), enqueue an
///   INLINE-callback task carrying `(cb_f, value, next)`, and return
///   `next`. Skips Promise #1 entirely. The microtask runner's
///   `Task::Inline` arm dispatches the callback and propagates its
///   return value to `next` exactly as the legacy two-promise path
///   would have.
///
/// Falls back to the unfused sequence when `value` is a real Promise
/// or could be a thenable — those need the proper assimilation path.
#[no_mangle]
pub extern "C" fn js_promise_resolved_then(
    value: f64,
    on_fulfilled: ClosurePtr,
    on_rejected: ClosurePtr,
) -> *mut Promise {
    if is_definitely_primitive(value) {
        MT_FAST_PATH_HIT.fetch_add(1, Ordering::Relaxed);
        // FAST PATH (primitive) — skip Promise.resolve()'s allocation
        // entirely. The callback runs once during the next microtask
        // drain via the `Task::Inline` arm.
        let next = js_promise_new();
        TASK_QUEUE.with(|q| {
            q.borrow_mut()
                .push_back(Task::Inline(on_fulfilled, value, next, true));
        });
        crate::event_pump::js_notify_main_thread();
        // Suppress the rejection-handler bookkeeping: it would only
        // matter if `value` were a Promise, which it isn't here.
        let _ = on_rejected;
        return next;
    }

    // FAST PATH (already-a-Promise) — `Promise.resolve(P)` is the
    // identity per spec when P is a real Promise (the same constructor
    // is used). The async-to-generator transform's per-`await` lowering
    // routes through `Promise.resolve(__step_r.value).then(...)`; in
    // the steady state `__step_r.value` is a Promise that the user's
    // `await <expr>` produced, so the wrap-then-unwrap is wasted work.
    // Skip the wrapper allocation: directly chain `.then()` off the
    // existing promise.
    if js_value_is_promise(value) != 0 {
        MT_FAST_PATH_HIT.fetch_add(1, Ordering::Relaxed);
        let inner = crate::value::js_nanbox_get_pointer(value) as *mut Promise;
        if !inner.is_null() {
            return js_promise_then(inner, on_fulfilled, on_rejected);
        }
    }

    MT_FAST_PATH_MISS.fetch_add(1, Ordering::Relaxed);
    // Pointer-tagged but not a Promise — could be a thenable. Take
    // the unfused path so the assimilation probes can run on it.
    let p1 = js_promise_resolved(value);
    js_promise_then(p1, on_fulfilled, on_rejected)
}

/// Specialized version of `js_promise_resolved_then` for the async-step
/// driver's per-`await` lowering: equivalent to
/// `Promise.resolve(value).then(v => step(v, false), e => step(e, true))`
/// but skips the `then_v_arrow` / `then_e_arrow` wrapper closures
/// entirely by carrying `step_closure` directly through the task queue
/// and invoking it as a 2-arg call at dispatch time.
///
/// Primary perf wins:
///   - One fewer closure allocation per await on the primitive-value
///     fast path (was: `Task::Inline` carrying `then_v_arrow`; is now
///     `Task::AsyncStep` carrying `step_closure`).
///   - One fewer closure dispatch per microtask (was: `then_v_arrow`
///     body called `step_closure`; is now `step_closure` is invoked
///     directly by the runner).
#[no_mangle]
pub extern "C" fn js_async_step_chain(
    value: f64,
    step_closure: ClosurePtr,
) -> *mut Promise {
    if is_definitely_primitive(value) {
        MT_FAST_PATH_HIT.fetch_add(1, Ordering::Relaxed);
        let next = js_promise_new();
        TASK_QUEUE.with(|q| {
            q.borrow_mut()
                .push_back(Task::AsyncStep(step_closure, value, next, false));
        });
        crate::event_pump::js_notify_main_thread();
        return next;
    }
    // For the promise/thenable case, fall back to the legacy
    // `Promise.resolve(value).then(then_v_arrow, then_e_arrow)` shape
    // — we don't have wrapper closures here, so we can't skip them.
    // The codegen emits this case rarely (await-of-a-Promise is a
    // small fraction of the steady state; primitive awaits dominate),
    // so the slow path is not yet specialized. To avoid breaking
    // dispatch we just panic — codegen will never emit this call
    // with a non-primitive `value` (see lower_call.rs).
    //
    // NOTE: this branch is unreachable from the codegen, but kept
    // for ABI safety if future code calls this directly.
    let (fulfill, reject) = build_async_step_thunks(step_closure);
    if js_value_is_promise(value) != 0 {
        let inner = crate::value::js_nanbox_get_pointer(value) as *mut Promise;
        if !inner.is_null() {
            return js_promise_then(inner, fulfill, reject);
        }
    }
    let p = js_promise_resolved(value);
    js_promise_then(p, fulfill, reject)
}

// Thread-local single-slot cache for async-step thunks. Keyed by the
// step closure pointer. When the same step closure is used across
// multiple promise-of-promise awaits (the simple-probe shape), we
// return the cached thunks; otherwise we allocate. The thunks are
// GC-rooted via `ASYNC_STEP_THUNK_CACHE_SCANNER` so they survive
// collection until evicted by a different step closure.
thread_local! {
    static LAST_ASYNC_STEP_THUNKS: std::cell::Cell<(usize, *mut crate::closure::ClosureHeader, *mut crate::closure::ClosureHeader)> =
        const { std::cell::Cell::new((0, std::ptr::null_mut(), std::ptr::null_mut())) };
}

/// GC root scanner for the LAST_ASYNC_STEP_THUNKS cache.
pub fn scan_async_step_thunk_cache(mark: &mut dyn FnMut(f64)) {
    let (_, f, r) = LAST_ASYNC_STEP_THUNKS.with(|c| c.get());
    if !f.is_null() {
        let boxed = f64::from_bits(
            0x7FFD_0000_0000_0000 | (f as u64 & 0x0000_FFFF_FFFF_FFFF),
        );
        mark(boxed);
    }
    if !r.is_null() {
        let boxed = f64::from_bits(
            0x7FFD_0000_0000_0000 | (r as u64 & 0x0000_FFFF_FFFF_FFFF),
        );
        mark(boxed);
    }
}

/// Build (fulfill, reject) thunks for the async-step promise-chain
/// fallback. Uses LAST_ASYNC_STEP_THUNKS as a single-slot cache —
/// hits 100% in the simple-probe shape (one step closure across all
/// awaits) while degrading gracefully (no cache overhead beyond the
/// cell read/write) when many distinct step closures interleave (the
/// Promise.all-of-N shape).
fn build_async_step_thunks(
    step_closure: ClosurePtr,
) -> (ClosurePtr, ClosurePtr) {
    let key = step_closure as usize;
    let cached = LAST_ASYNC_STEP_THUNKS.with(|c| c.get());
    if cached.0 == key && !cached.1.is_null() && !cached.2.is_null() {
        return (cached.1 as ClosurePtr, cached.2 as ClosurePtr);
    }
    use crate::closure::{js_closure_alloc, js_closure_set_capture_ptr};
    let fulfill = js_closure_alloc(async_step_fulfill_thunk as *const u8, 1);
    js_closure_set_capture_ptr(fulfill, 0, step_closure as i64);
    let reject = js_closure_alloc(async_step_reject_thunk as *const u8, 1);
    js_closure_set_capture_ptr(reject, 0, step_closure as i64);
    LAST_ASYNC_STEP_THUNKS.with(|c| c.set((key, fulfill, reject)));
    (fulfill as ClosurePtr, reject as ClosurePtr)
}

// Thunks for the promise/thenable fallback in js_async_step_chain.
// Capture layout: [step_closure_ptr]
extern "C" fn async_step_fulfill_thunk(
    closure: *const crate::closure::ClosureHeader,
    value: f64,
) -> f64 {
    let step = crate::closure::js_closure_get_capture_ptr(closure, 0)
        as *const crate::closure::ClosureHeader;
    let false_bits = f64::from_bits(0x7FFC_0000_0000_0003);
    crate::closure::js_closure_call2(step, value, false_bits)
}

extern "C" fn async_step_reject_thunk(
    closure: *const crate::closure::ClosureHeader,
    value: f64,
) -> f64 {
    let step = crate::closure::js_closure_get_capture_ptr(closure, 0)
        as *const crate::closure::ClosureHeader;
    let true_bits = f64::from_bits(0x7FFC_0000_0000_0004);
    crate::closure::js_closure_call2(step, value, true_bits)
}

/// `Array.fromAsync(input)` — Node 22+ static method.
///
/// Returns a Promise that resolves to an Array. Two input shapes:
///   1. **Array**: each element is awaited (if it's a Promise) and the
///      results are collected. Equivalent to `Promise.all(input)`.
///   2. **Async iterator** (object with a `.next()` method): we call
///      `.next()` repeatedly via the closure-chained .then() pattern,
///      pushing each `value` until `done` is true, then resolve the
///      output Promise with the collected array.
///
/// `input` is the NaN-boxed input value. Returns a NaN-boxed Promise
/// pointer (POINTER_TAG) so the caller's `await` can unwrap it.
#[no_mangle]
pub extern "C" fn js_array_from_async(input: f64) -> f64 {
    use crate::array::{js_array_alloc, ArrayHeader};
    use crate::closure::{js_closure_alloc, js_closure_set_capture_ptr};
    use crate::value::js_nanbox_get_pointer;

    // Strip NaN-box to get the raw pointer.
    let raw_ptr = js_nanbox_get_pointer(input) as usize;
    if raw_ptr == 0 {
        // null/undefined input — resolve to empty array
        let empty = js_array_alloc(0);
        unsafe {
            (*empty).length = 0;
        }
        let arr_f64 = crate::value::js_nanbox_pointer(empty as i64);
        let p = js_promise_resolved(arr_f64);
        return crate::value::js_nanbox_pointer(p as i64);
    }

    // Path 1: input is an Array. Reuse Promise.all behavior — js_promise_all
    // handles a mix of promise and non-promise elements correctly.
    unsafe {
        let gc_header =
            (raw_ptr as *const u8).sub(crate::gc::GC_HEADER_SIZE) as *const crate::gc::GcHeader;
        if (*gc_header).obj_type == crate::gc::GC_TYPE_ARRAY {
            let arr_ptr = raw_ptr as *const ArrayHeader;
            let p = js_promise_all(arr_ptr);
            return crate::value::js_nanbox_pointer(p as i64);
        }
    }

    // Path 2: async iterator (or any other object). Allocate a result
    // Promise and an empty result Array, then kick off the .next() chain.
    let result_promise = js_promise_new();
    let result_arr = js_array_alloc(0);
    unsafe {
        (*result_arr).length = 0;
    }

    // Build the recursive .next() handler closure. Captures:
    //   [0] result_promise (Promise to resolve at the end)
    //   [1] result_arr (Array to push each value into)
    //   [2] iter object (raw pointer; we re-NaN-box on .next() call)
    let chain_closure = js_closure_alloc(array_from_async_step as *const u8, 3);
    js_closure_set_capture_ptr(chain_closure, 0, result_promise as i64);
    js_closure_set_capture_ptr(chain_closure, 1, result_arr as i64);
    js_closure_set_capture_ptr(chain_closure, 2, raw_ptr as i64);

    // Kick off the first .next() call. The handler returns the iter result
    // (or undefined for done) — we wire it through `.then(chain_closure)`
    // which will recurse.
    unsafe {
        array_from_async_call_next(raw_ptr, chain_closure);
    }

    crate::value::js_nanbox_pointer(result_promise as i64)
}

/// Helper that calls `iter.next()` (returning a Promise) and attaches
/// `chain_closure` as both fulfill and reject handlers. Used by the async
/// iterator path of `js_array_from_async`.
unsafe fn array_from_async_call_next(
    iter_ptr: usize,
    chain_closure: *const crate::closure::ClosureHeader,
) {
    // Re-NaN-box the iter pointer for js_native_call_method.
    let iter_f64 = crate::value::js_nanbox_pointer(iter_ptr as i64);
    let method_name = b"next";
    // Call iter.next() — returns a Promise<{value, done}> for async generators
    // or `{value, done}` directly for sync iterators.
    let next_result = crate::object::js_native_call_method(
        iter_f64,
        method_name.as_ptr() as *const i8,
        method_name.len(),
        std::ptr::null(),
        0,
    );

    // If the result is a Promise pointer, attach the handler via .then.
    let next_ptr = crate::value::js_nanbox_get_pointer(next_result) as usize;
    if next_ptr != 0 {
        let gc_header =
            (next_ptr as *const u8).sub(crate::gc::GC_HEADER_SIZE) as *const crate::gc::GcHeader;
        if (*gc_header).obj_type == crate::gc::GC_TYPE_PROMISE {
            let next_promise = next_ptr as *mut Promise;
            // Use the chain_closure for both fulfill and reject. On rejection
            // we just propagate by resolving the result_promise with undefined.
            js_promise_then(
                next_promise,
                chain_closure as *const _,
                chain_closure as *const _,
            );
            return;
        }
    }
    // Synchronous iterator path: invoke the handler directly with the
    // result so the iteration loop continues without going through .then.
    array_from_async_step(chain_closure as *const _, next_result);
}

/// `.then(...)` handler invoked once per `.next()` resolution. Reads the
/// `{value, done}` iter-result, pushes `value` into the accumulator array,
/// and either resolves the output Promise (when `done`) or schedules
/// another `.next()` call.
extern "C" fn array_from_async_step(
    closure: *const crate::closure::ClosureHeader,
    iter_result: f64,
) -> f64 {
    use crate::array::{js_array_push_f64, ArrayHeader};
    use crate::closure::{js_closure_get_capture_ptr, js_closure_set_capture_ptr};

    let result_promise = js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    let mut result_arr = js_closure_get_capture_ptr(closure, 1) as *mut ArrayHeader;
    let iter_ptr = js_closure_get_capture_ptr(closure, 2) as usize;

    if result_promise.is_null() || result_arr.is_null() || iter_ptr == 0 {
        return 0.0;
    }

    // Read `done` and `value` off the iter result. The result is either an
    // object with those fields, or undefined (if next() returned undefined).
    let result_bits = iter_result.to_bits();
    let result_obj_ptr = if (result_bits >> 48) == 0x7FFD {
        (result_bits & 0x0000_FFFF_FFFF_FFFF) as *const crate::object::ObjectHeader
    } else if result_bits != 0 && result_bits <= 0x0000_FFFF_FFFF_FFFF {
        result_bits as *const crate::object::ObjectHeader
    } else {
        // Treat malformed result as `done: true`.
        std::ptr::null()
    };

    if result_obj_ptr.is_null() {
        // No more values — resolve the output promise with the collected array.
        let arr_f64 = crate::value::js_nanbox_pointer(result_arr as i64);
        unsafe {
            js_promise_resolve(result_promise, arr_f64);
        }
        return 0.0;
    }

    // Look up "done" and "value" fields by name.
    let done_key = make_static_string(b"done");
    let value_key = make_static_string(b"value");
    let done_jv = unsafe { crate::object::js_object_get_field_by_name(result_obj_ptr, done_key) };
    let value_jv = unsafe { crate::object::js_object_get_field_by_name(result_obj_ptr, value_key) };
    let done_f64 = f64::from_bits(done_jv.bits());
    let value_f64 = f64::from_bits(value_jv.bits());

    if crate::value::js_is_truthy(done_f64) != 0 {
        // Iteration complete — resolve with the accumulated array.
        let arr_f64 = crate::value::js_nanbox_pointer(result_arr as i64);
        unsafe {
            js_promise_resolve(result_promise, arr_f64);
        }
        return 0.0;
    }

    // Push the value (push_f64 may grow & return a new pointer).
    result_arr = js_array_push_f64(result_arr, value_f64);
    // Update the closure capture so subsequent steps see the (possibly
    // moved) array.
    js_closure_set_capture_ptr(closure as *mut _, 1, result_arr as i64);

    // Recurse: call iter.next() again. The same closure will be invoked
    // when the next promise resolves.
    unsafe {
        array_from_async_call_next(iter_ptr, closure);
    }

    0.0
}

/// Helper to allocate a static StringHeader for property-name lookups.
/// Reuses `js_string_from_bytes` so the result is GC-tracked.
fn make_static_string(bytes: &[u8]) -> *const crate::string::StringHeader {
    crate::string::js_string_from_bytes(bytes.as_ptr(), bytes.len() as u32)
}

/// Create a rejected promise with the given reason
#[no_mangle]
pub extern "C" fn js_promise_rejected(reason: f64) -> *mut Promise {
    let promise = js_promise_new();
    js_promise_reject(promise, reason);
    promise
}

/// Check if a value is a promise (by checking if it's a valid pointer)
/// This is a simplified check - in reality we'd need type tags
#[no_mangle]
pub extern "C" fn js_is_promise(ptr: *mut Promise) -> i32 {
    if ptr.is_null() {
        return 0;
    }
    // Basic sanity check - could be more sophisticated
    1
}

/// Safe `await`-side check: given a NaN-boxed JSValue, return 1 if it
/// points at a real Promise allocation and 0 otherwise. Used by the
/// LLVM backend's `Expr::Await` lowering so that `await <non-promise>`
/// doesn't dereference a garbage pointer as if it were a `Promise`.
///
/// Inspects the NaN-box tag and, when the value is a pointer, walks
/// back to the `GcHeader` to read the `obj_type`. Any non-POINTER_TAG
/// bits (primitives, strings, bigints, null, undefined) return 0.
#[no_mangle]
pub extern "C" fn js_value_is_promise(value: f64) -> i32 {
    const POINTER_TAG: u64 = 0x7FFD_0000_0000_0000;
    const TAG_MASK: u64 = 0xFFFF_0000_0000_0000;
    const POINTER_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

    let bits = value.to_bits();
    let tag = bits & TAG_MASK;
    if tag != POINTER_TAG {
        return 0;
    }
    let ptr_usize = (bits & crate::value::POINTER_MASK) as usize;
    if ptr_usize < 0x10000 {
        return 0;
    }
    unsafe {
        let gc_header =
            (ptr_usize as *const u8).sub(crate::gc::GC_HEADER_SIZE) as *const crate::gc::GcHeader;
        let ot = (*gc_header).obj_type;
        if ot == crate::gc::GC_TYPE_PROMISE {
            1
        } else {
            0
        }
    }
}

// Queue for scheduled promise resolutions
thread_local! {
    static SCHEDULED_RESOLVES: RefCell<Vec<(*mut Promise, f64)>> = const { RefCell::new(Vec::new()) };
}

/// Schedule a promise to be resolved with a value when microtasks run
/// This simulates an async operation completing
#[no_mangle]
pub extern "C" fn js_promise_schedule_resolve(promise: *mut Promise, value: f64) {
    SCHEDULED_RESOLVES.with(|q| {
        q.borrow_mut().push((promise, value));
    });
}

/// Process scheduled resolutions (called by js_promise_run_microtasks)
fn process_scheduled_resolves() -> i32 {
    let mut count = 0;
    loop {
        let item = SCHEDULED_RESOLVES.with(|q| q.borrow_mut().pop());
        match item {
            Some((promise, value)) => {
                js_promise_resolve(promise, value);
                count += 1;
            }
            None => break,
        }
    }
    count
}

/// Create a new Promise with an executor callback.
/// The executor receives (resolve, reject) as arguments.
/// resolve and reject are closures that call js_promise_resolve/js_promise_reject.
///
/// Arguments:
/// - executor: A closure that takes 2 arguments (resolve_fn, reject_fn)
#[no_mangle]
pub extern "C" fn js_promise_new_with_executor(
    executor: *const crate::closure::ClosureHeader,
) -> *mut Promise {
    use crate::closure::{js_closure_alloc, js_closure_call2, js_closure_set_capture_ptr};

    let promise = js_promise_new();
    let promise_i64 = promise as i64;

    // Create resolve closure that captures the promise pointer
    // The resolve function signature is: (closure: *const ClosureHeader, value: f64) -> f64
    let resolve_closure = js_closure_alloc(promise_resolve_fn as *const u8, 1);
    js_closure_set_capture_ptr(resolve_closure, 0, promise_i64);

    // Create reject closure that captures the promise pointer
    let reject_closure = js_closure_alloc(promise_reject_fn as *const u8, 1);
    js_closure_set_capture_ptr(reject_closure, 0, promise_i64);

    // Call the executor with (resolve_closure, reject_closure)
    // The closures are passed as f64 by bitcasting the pointer bits
    // This preserves the exact bits of the pointer when passed through f64 ABI
    let resolve_f64: f64 = unsafe { f64::from_bits(i64::cast_unsigned(resolve_closure as i64)) };
    let reject_f64: f64 = unsafe { f64::from_bits(i64::cast_unsigned(reject_closure as i64)) };
    unsafe {
        js_closure_call2(executor, resolve_f64, reject_f64);
    }

    promise
}

/// Internal resolve function for Promise executor callbacks.
/// Called when user calls resolve(value) inside the executor.
extern "C" fn promise_resolve_fn(closure: *const crate::closure::ClosureHeader, value: f64) -> f64 {
    use crate::closure::js_closure_get_capture_ptr;

    let promise_ptr = js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    js_promise_resolve(promise_ptr, value);
    0.0 // resolve returns undefined
}

/// Internal reject function for Promise executor callbacks.
/// Called when user calls reject(reason) inside the executor.
extern "C" fn promise_reject_fn(closure: *const crate::closure::ClosureHeader, reason: f64) -> f64 {
    use crate::closure::js_closure_get_capture_ptr;

    let promise_ptr = js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    js_promise_reject(promise_ptr, reason);
    0.0 // reject returns undefined
}

/// Promise.all - takes an array of promises and returns a promise that resolves
/// with an array of all resolved values, or rejects if any promise rejects.
///
/// Arguments:
/// - promises_arr: pointer to an ArrayHeader containing promise pointers (as NaN-boxed f64)
///
/// Returns: a new Promise that resolves with an array of results
#[no_mangle]
pub extern "C" fn js_promise_all(promises_arr: *const crate::array::ArrayHeader) -> *mut Promise {
    use crate::array::{js_array_alloc, js_array_get_f64, js_array_length, js_array_set_f64};
    use crate::closure::{
        js_closure_alloc, js_closure_set_capture_f64, js_closure_set_capture_ptr,
    };
    use crate::value::js_nanbox_get_pointer;

    // Create the result promise
    let result_promise = js_promise_new();

    if promises_arr.is_null() {
        // Promise.all([]) resolves immediately with empty array
        let empty_arr = js_array_alloc(0);
        unsafe {
            (*empty_arr).length = 0;
        }
        let arr_f64 = crate::value::js_nanbox_pointer(empty_arr as i64);
        js_promise_resolve(result_promise, arr_f64);
        return result_promise;
    }

    let count = js_array_length(promises_arr);

    if count == 0 {
        // Promise.all([]) resolves immediately with empty array
        let empty_arr = js_array_alloc(0);
        unsafe {
            (*empty_arr).length = 0;
        }
        let arr_f64 = crate::value::js_nanbox_pointer(empty_arr as i64);
        js_promise_resolve(result_promise, arr_f64);
        return result_promise;
    }

    // Allocate result array to hold resolved values
    let results_arr = js_array_alloc(count);
    unsafe {
        (*results_arr).length = count;
    }

    // Initialize all elements to undefined
    const TAG_UNDEFINED: u64 = 0x7FFC_0000_0000_0001;
    for i in 0..count {
        js_array_set_f64(results_arr, i, f64::from_bits(TAG_UNDEFINED));
    }

    // Allocate state: [remaining_count, rejected_flag]
    // We use an array to hold mutable shared state across closures
    let state_arr = js_array_alloc(2);
    unsafe {
        (*state_arr).length = 2;
    }
    js_array_set_f64(state_arr, 0, count as f64); // remaining count
    js_array_set_f64(state_arr, 1, 0.0); // rejected flag (0 = not rejected)

    // For each promise in the array, attach a .then handler
    for i in 0..count {
        let promise_f64 = js_array_get_f64(promises_arr, i);

        // Discriminate via the GC-header obj_type, not via raw pointer
        // extraction: string/bigint NaN-boxed values produce non-null
        // pointers from js_nanbox_get_pointer and would be passed to
        // js_promise_then as if they were Promises.
        if js_value_is_promise(promise_f64) == 0 {
            // Not a promise — treat as already resolved value
            js_array_set_f64(results_arr, i, promise_f64);
            let remaining = js_array_get_f64(state_arr, 0) - 1.0;
            js_array_set_f64(state_arr, 0, remaining);
            continue;
        }

        let promise_ptr = js_nanbox_get_pointer(promise_f64) as *mut Promise;

        // Create fulfill closure for this promise
        // Captures: [result_promise, results_arr, state_arr, index]
        let fulfill_closure = js_closure_alloc(promise_all_fulfill_handler as *const u8, 4);
        js_closure_set_capture_ptr(fulfill_closure, 0, result_promise as i64);
        js_closure_set_capture_ptr(fulfill_closure, 1, results_arr as i64);
        js_closure_set_capture_ptr(fulfill_closure, 2, state_arr as i64);
        js_closure_set_capture_f64(fulfill_closure, 3, i as f64);

        // Create reject closure for this promise
        // Captures: [result_promise, state_arr]
        let reject_closure = js_closure_alloc(promise_all_reject_handler as *const u8, 2);
        js_closure_set_capture_ptr(reject_closure, 0, result_promise as i64);
        js_closure_set_capture_ptr(reject_closure, 1, state_arr as i64);

        // Attach handlers to the promise WITHOUT allocating a `next`
        // Promise — the return of `then` is unused here. Saves one
        // promise alloc per input on Promise.all (50k for the
        // 1000-batch × 50-input bench).
        js_promise_attach_handlers(promise_ptr, fulfill_closure, reject_closure);
    }

    // Check if all were non-promises (already resolved)
    let remaining = js_array_get_f64(state_arr, 0);
    if remaining == 0.0 {
        let arr_f64 = crate::value::js_nanbox_pointer(results_arr as i64);
        js_promise_resolve(result_promise, arr_f64);
    }

    result_promise
}

/// Internal handler called when a promise in Promise.all fulfills
extern "C" fn promise_all_fulfill_handler(
    closure: *const crate::closure::ClosureHeader,
    value: f64,
) -> f64 {
    use crate::array::{js_array_get_f64, js_array_set_f64, ArrayHeader};
    use crate::closure::{js_closure_get_capture_f64, js_closure_get_capture_ptr};

    let result_promise = js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    let results_arr = js_closure_get_capture_ptr(closure, 1) as *mut ArrayHeader;
    let state_arr = js_closure_get_capture_ptr(closure, 2) as *mut ArrayHeader;
    if result_promise.is_null() || results_arr.is_null() || state_arr.is_null() {
        return 0.0;
    }
    let index = js_closure_get_capture_f64(closure, 3) as u32;

    // Check if already rejected
    let rejected = js_array_get_f64(state_arr, 1);
    if rejected != 0.0 {
        return 0.0;
    }

    // Store the resolved value
    js_array_set_f64(results_arr, index, value);

    // Decrement remaining count
    let remaining = js_array_get_f64(state_arr, 0) - 1.0;
    js_array_set_f64(state_arr, 0, remaining);

    // If all promises have resolved, resolve the result promise with the array
    if remaining == 0.0 {
        let arr_f64 = crate::value::js_nanbox_pointer(results_arr as i64);
        js_promise_resolve(result_promise, arr_f64);
    }

    0.0
}

/// Internal handler called when a promise in Promise.all rejects
extern "C" fn promise_all_reject_handler(
    closure: *const crate::closure::ClosureHeader,
    reason: f64,
) -> f64 {
    use crate::array::{js_array_get_f64, js_array_set_f64, ArrayHeader};
    use crate::closure::js_closure_get_capture_ptr;

    let result_promise = js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    let state_arr = js_closure_get_capture_ptr(closure, 1) as *mut ArrayHeader;
    if result_promise.is_null() || state_arr.is_null() {
        return 0.0;
    }

    // Check if already rejected (only reject once)
    let rejected = js_array_get_f64(state_arr, 1);
    if rejected != 0.0 {
        return 0.0;
    }

    // Mark as rejected
    js_array_set_f64(state_arr, 1, 1.0);

    // Reject the result promise with the reason
    js_promise_reject(result_promise, reason);

    0.0
}

/// Promise.race - takes an array of promises and returns a promise that resolves
/// or rejects with the first promise that settles.
#[no_mangle]
pub extern "C" fn js_promise_race(promises_arr: *const crate::array::ArrayHeader) -> *mut Promise {
    use crate::array::{js_array_get_f64, js_array_length};
    use crate::closure::{js_closure_alloc, js_closure_set_capture_ptr};
    use crate::value::js_nanbox_get_pointer;

    let result_promise = js_promise_new();

    if promises_arr.is_null() {
        // Promise.race([]) — never settles (per spec), but return pending promise
        return result_promise;
    }

    let count = js_array_length(promises_arr);
    if count == 0 {
        return result_promise;
    }

    // For each promise, attach resolve/reject handlers that settle the result promise.
    // Per the spec, even when an input promise is already settled we MUST route the
    // resolution through the microtask queue (by registering `.then` handlers) rather
    // than calling js_promise_resolve synchronously.  The synchronous short-circuit was
    // causing race / any results to appear too early in the output when compared against
    // Node's microtask-ordered output.
    for i in 0..count {
        let promise_f64 = js_array_get_f64(promises_arr, i);
        // Discriminate via GC-header obj_type — string/bigint NaN-boxed
        // values would otherwise pass through pointer extraction and crash
        // js_promise_then.
        if js_value_is_promise(promise_f64) == 0 {
            // Non-promise value — wrap as an already-resolved promise so the
            // resolution goes through the normal microtask path.
            let wrapped = js_promise_resolved(promise_f64);
            let resolve_closure = js_closure_alloc(promise_race_resolve_handler as *const u8, 1);
            js_closure_set_capture_ptr(resolve_closure, 0, result_promise as i64);
            let reject_closure = js_closure_alloc(promise_race_reject_handler as *const u8, 1);
            js_closure_set_capture_ptr(reject_closure, 0, result_promise as i64);
            js_promise_then(wrapped, resolve_closure, reject_closure);
            continue;
        }
        let promise_ptr = js_nanbox_get_pointer(promise_f64) as *mut Promise;

        // Always use js_promise_then even for already-settled promises.
        // This ensures race resolution goes through the microtask queue,
        // matching the spec-mandated ordering vs other pending callbacks.
        let resolve_closure = js_closure_alloc(
            promise_race_resolve_handler as *const u8,
            1, // 1 capture: result_promise
        );
        js_closure_set_capture_ptr(resolve_closure, 0, result_promise as i64);

        // Create reject handler closure (captures result_promise)
        let reject_closure = js_closure_alloc(promise_race_reject_handler as *const u8, 1);
        js_closure_set_capture_ptr(reject_closure, 0, result_promise as i64);

        // Attach handlers via then — if the input is already settled this will
        // push a microtask rather than resolving result_promise synchronously.
        js_promise_attach_handlers(promise_ptr, resolve_closure, reject_closure);
    }

    result_promise
}

/// Handler for Promise.race fulfill — resolves the race promise with the first value
extern "C" fn promise_race_resolve_handler(
    closure: *const crate::closure::ClosureHeader,
    value: f64,
) -> f64 {
    use crate::closure::js_closure_get_capture_ptr;
    let result_promise = js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    if result_promise.is_null() {
        return 0.0;
    }
    // Only settle if still pending (first one wins)
    if matches!(unsafe { (*result_promise).state }, PromiseState::Pending) {
        js_promise_resolve(result_promise, value);
    }
    0.0
}

/// Handler for Promise.race reject — rejects the race promise with the first reason
extern "C" fn promise_race_reject_handler(
    closure: *const crate::closure::ClosureHeader,
    reason: f64,
) -> f64 {
    use crate::closure::js_closure_get_capture_ptr;
    let result_promise = js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    if result_promise.is_null() {
        return 0.0;
    }
    if matches!(unsafe { (*result_promise).state }, PromiseState::Pending) {
        js_promise_reject(result_promise, reason);
    }
    0.0
}

/// Await any promise value.
/// In native-only mode (no V8), all promises are native POINTER_TAG promises.
/// The codegen-emitted busy-wait loop handles polling the promise state,
/// so we just return the value as-is.
/// In V8 mode (perry-jsruntime), this function is overridden by the V8-aware
/// version that can also handle JS_HANDLE_TAG promises.
#[no_mangle]
pub extern "C" fn js_await_any_promise(value: f64) -> f64 {
    value
}

/// ECMAScript thenable assimilation for `await`. Issue #586.
///
/// `await x` semantics: if `x` is an object with a callable `then` method,
/// the runtime should call `x.then(resolve, reject)` and resume with whatever
/// the underlying then implementation passes to `resolve`. Real Promises take
/// the fast path; thenables (e.g. drizzle-orm's `QueryPromise`) need this.
///
/// Behavior:
/// - Already a Promise → pass through unchanged (caller's await loop polls it).
/// - Object whose class chain contains a `then(onFulfilled, onRejected)` method
///   → allocate a fresh Promise, build resolve/reject closures bound to it,
///     invoke `value.then(resolve, reject)`, and return the new Promise (which
///     the await loop then polls). When the user's `then` calls `resolve(v)`,
///     our handler resolves the wrapper promise; the await loop sees Fulfilled
///     and returns `v`.
/// - Anything else (primitives, plain objects without a `then` method, Map /
///   Set / Buffer / handle values) → pass through unchanged so the await
///   resolves with the value itself per spec.
///
/// `then` is looked up only in the class vtable, not as an instance field.
/// Object literals with a `then: () => ...` arrow stored as a property are
/// uncommon in practice and would require a parallel `js_object_get_field_by_name`
/// probe — out of scope for this fix.
#[no_mangle]
pub extern "C" fn js_assimilate_thenable(value: f64) -> f64 {
    use crate::value::JSValue;

    MT_THENABLE_PROBE_COUNT.fetch_add(1, Ordering::Relaxed);
    // Real Promise — caller's await loop already handles it.
    if js_value_is_promise(value) != 0 {
        return value;
    }

    let bits = value.to_bits();
    let jsval = JSValue::from_bits(bits);

    if !jsval.is_pointer() {
        return value;
    }

    let raw_ptr = (bits & 0x0000_FFFF_FFFF_FFFF) as usize;
    if raw_ptr < 0x100000 {
        return value;
    }

    // Side-table-tracked heap types don't have ClassVTable entries; skip.
    if crate::buffer::is_registered_buffer(raw_ptr)
        || crate::set::is_registered_set(raw_ptr)
        || crate::map::is_registered_map(raw_ptr)
        || crate::symbol::is_registered_symbol(raw_ptr)
        || crate::regex::is_regex_pointer(raw_ptr as *const u8)
        || crate::date::is_registered_date_bits(bits)
    {
        return value;
    }

    let obj_ptr = jsval.as_pointer::<crate::object::ObjectHeader>();
    if obj_ptr.is_null() {
        return value;
    }

    // Verify GC type before reading class_id; reading garbage past random
    // pointers would either return a fake match or segfault.
    let class_id = unsafe {
        let gc_header = (obj_ptr as *const u8).sub(crate::gc::GC_HEADER_SIZE)
            as *const crate::gc::GcHeader;
        let gc_type = (*gc_header).obj_type;
        if gc_type != crate::gc::GC_TYPE_OBJECT {
            return value;
        }
        (*obj_ptr).class_id
    };
    if class_id == 0 {
        return value;
    }

    // Probe the vtable chain for `then`. Bail out on plain objects (no class
    // method) so the await passes the original value through unchanged.
    let (then_func_ptr, then_param_count) =
        match crate::object::lookup_class_method_in_chain(class_id, "then") {
            Some(p) => p,
            None => return value,
        };

    // Allocate the wrapper promise plus resolve/reject closures pointing at it.
    let new_promise = js_promise_new();
    let promise_i64 = new_promise as i64;

    let resolve_closure =
        crate::closure::js_closure_alloc(promise_resolve_fn as *const u8, 1);
    crate::closure::js_closure_set_capture_ptr(resolve_closure, 0, promise_i64);
    let reject_closure =
        crate::closure::js_closure_alloc(promise_reject_fn as *const u8, 1);
    crate::closure::js_closure_set_capture_ptr(reject_closure, 0, promise_i64);

    // The user's `then(onFulfilled, onRejected)` reads each parameter as a
    // raw f64 closure pointer (matching the convention used by
    // `js_promise_new_with_executor`).
    let resolve_f64 = f64::from_bits(resolve_closure as u64);
    let reject_f64 = f64::from_bits(reject_closure as u64);

    // Invoke `value.then(resolve, reject)` via the vtable. Mirrors
    // `call_vtable_method` in object.rs: NaN-box `this` with POINTER_TAG so
    // the method body sees a real instance pointer.
    let this_f64 = f64::from_bits(JSValue::pointer(obj_ptr as *mut u8).bits());
    unsafe {
        match then_param_count {
            0 => {
                let f: extern "C" fn(f64) -> f64 = std::mem::transmute(then_func_ptr);
                f(this_f64);
            }
            1 => {
                let f: extern "C" fn(f64, f64) -> f64 = std::mem::transmute(then_func_ptr);
                f(this_f64, resolve_f64);
            }
            _ => {
                // 2+ params: pass resolve/reject; any extra slots arrive as NaN.
                let f: extern "C" fn(f64, f64, f64) -> f64 =
                    std::mem::transmute(then_func_ptr);
                f(this_f64, resolve_f64, reject_f64);
            }
        }
    }

    crate::value::js_nanbox_pointer(new_promise as i64)
}

/// Build a `{ status: "fulfilled", value: v }` object for Promise.allSettled.
fn build_settled_fulfilled(value: f64) -> f64 {
    use crate::object::{js_object_alloc_with_shape, js_object_set_field};
    let packed = b"status\0value\0";
    let obj = js_object_alloc_with_shape(0x7FFF_FF10, 2, packed.as_ptr(), packed.len() as u32);
    let status_str = crate::string::js_string_from_bytes(b"fulfilled".as_ptr(), 9);
    let status_nb = crate::value::js_nanbox_string(status_str as i64);
    js_object_set_field(
        obj,
        0,
        crate::value::JSValue::from_bits(status_nb.to_bits()),
    );
    js_object_set_field(obj, 1, crate::value::JSValue::from_bits(value.to_bits()));
    crate::value::js_nanbox_pointer(obj as i64)
}

/// Build a `{ status: "rejected", reason: r }` object for Promise.allSettled.
fn build_settled_rejected(reason: f64) -> f64 {
    use crate::object::{js_object_alloc_with_shape, js_object_set_field};
    let packed = b"status\0reason\0";
    let obj = js_object_alloc_with_shape(0x7FFF_FF11, 2, packed.as_ptr(), packed.len() as u32);
    let status_str = crate::string::js_string_from_bytes(b"rejected".as_ptr(), 8);
    let status_nb = crate::value::js_nanbox_string(status_str as i64);
    js_object_set_field(
        obj,
        0,
        crate::value::JSValue::from_bits(status_nb.to_bits()),
    );
    js_object_set_field(obj, 1, crate::value::JSValue::from_bits(reason.to_bits()));
    crate::value::js_nanbox_pointer(obj as i64)
}

/// Promise.allSettled — never rejects; resolves with an array of result objects
/// where each entry is `{ status: "fulfilled", value }` or `{ status: "rejected", reason }`.
#[no_mangle]
pub extern "C" fn js_promise_all_settled(
    promises_arr: *const crate::array::ArrayHeader,
) -> *mut Promise {
    use crate::array::{js_array_alloc, js_array_get_f64, js_array_length, js_array_set_f64};
    use crate::closure::{
        js_closure_alloc, js_closure_set_capture_f64, js_closure_set_capture_ptr,
    };
    use crate::value::js_nanbox_get_pointer;

    let result_promise = js_promise_new();

    if promises_arr.is_null() {
        let empty_arr = js_array_alloc(0);
        unsafe {
            (*empty_arr).length = 0;
        }
        let arr_f64 = crate::value::js_nanbox_pointer(empty_arr as i64);
        js_promise_resolve(result_promise, arr_f64);
        return result_promise;
    }

    let count = js_array_length(promises_arr);
    if count == 0 {
        let empty_arr = js_array_alloc(0);
        unsafe {
            (*empty_arr).length = 0;
        }
        let arr_f64 = crate::value::js_nanbox_pointer(empty_arr as i64);
        js_promise_resolve(result_promise, arr_f64);
        return result_promise;
    }

    let results_arr = js_array_alloc(count);
    unsafe {
        (*results_arr).length = count;
    }
    const TAG_UNDEFINED: u64 = 0x7FFC_0000_0000_0001;
    for i in 0..count {
        js_array_set_f64(results_arr, i, f64::from_bits(TAG_UNDEFINED));
    }

    // state: [remaining_count]
    let state_arr = js_array_alloc(1);
    unsafe {
        (*state_arr).length = 1;
    }
    js_array_set_f64(state_arr, 0, count as f64);

    for i in 0..count {
        let promise_f64 = js_array_get_f64(promises_arr, i);

        // Only treat as a Promise if the value is a POINTER_TAG that walks
        // back to a GcHeader with obj_type == GC_TYPE_PROMISE. Otherwise
        // (string, plain number, undefined, null, object, etc.) wrap the
        // value as already-fulfilled — Promise.allSettled spec passes any
        // non-thenable through as `{status: "fulfilled", value}`.
        let is_promise = js_value_is_promise(promise_f64) != 0;

        if !is_promise {
            // Non-promise value — wrap as fulfilled and decrement
            let wrapped = build_settled_fulfilled(promise_f64);
            js_array_set_f64(results_arr, i, wrapped);
            let remaining = js_array_get_f64(state_arr, 0) - 1.0;
            js_array_set_f64(state_arr, 0, remaining);
            continue;
        }

        let promise_ptr = js_nanbox_get_pointer(promise_f64) as *mut Promise;

        // Fulfill: store {status:"fulfilled", value:v}
        let fulfill_closure = js_closure_alloc(promise_all_settled_fulfill_handler as *const u8, 4);
        js_closure_set_capture_ptr(fulfill_closure, 0, result_promise as i64);
        js_closure_set_capture_ptr(fulfill_closure, 1, results_arr as i64);
        js_closure_set_capture_ptr(fulfill_closure, 2, state_arr as i64);
        js_closure_set_capture_f64(fulfill_closure, 3, i as f64);

        // Reject: store {status:"rejected", reason:r}
        let reject_closure = js_closure_alloc(promise_all_settled_reject_handler as *const u8, 4);
        js_closure_set_capture_ptr(reject_closure, 0, result_promise as i64);
        js_closure_set_capture_ptr(reject_closure, 1, results_arr as i64);
        js_closure_set_capture_ptr(reject_closure, 2, state_arr as i64);
        js_closure_set_capture_f64(reject_closure, 3, i as f64);

        js_promise_attach_handlers(promise_ptr, fulfill_closure, reject_closure);
    }

    // If all were already non-promises
    let remaining = js_array_get_f64(state_arr, 0);
    if remaining == 0.0 {
        let arr_f64 = crate::value::js_nanbox_pointer(results_arr as i64);
        js_promise_resolve(result_promise, arr_f64);
    }

    result_promise
}

extern "C" fn promise_all_settled_fulfill_handler(
    closure: *const crate::closure::ClosureHeader,
    value: f64,
) -> f64 {
    use crate::array::{js_array_get_f64, js_array_set_f64, ArrayHeader};
    use crate::closure::{js_closure_get_capture_f64, js_closure_get_capture_ptr};

    let result_promise = js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    let results_arr = js_closure_get_capture_ptr(closure, 1) as *mut ArrayHeader;
    let state_arr = js_closure_get_capture_ptr(closure, 2) as *mut ArrayHeader;
    if result_promise.is_null() || results_arr.is_null() || state_arr.is_null() {
        return 0.0;
    }
    let index = js_closure_get_capture_f64(closure, 3) as u32;

    let wrapped = build_settled_fulfilled(value);
    js_array_set_f64(results_arr, index, wrapped);

    let remaining = js_array_get_f64(state_arr, 0) - 1.0;
    js_array_set_f64(state_arr, 0, remaining);

    if remaining == 0.0 {
        let arr_f64 = crate::value::js_nanbox_pointer(results_arr as i64);
        js_promise_resolve(result_promise, arr_f64);
    }
    0.0
}

extern "C" fn promise_all_settled_reject_handler(
    closure: *const crate::closure::ClosureHeader,
    reason: f64,
) -> f64 {
    use crate::array::{js_array_get_f64, js_array_set_f64, ArrayHeader};
    use crate::closure::{js_closure_get_capture_f64, js_closure_get_capture_ptr};

    let result_promise = js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    let results_arr = js_closure_get_capture_ptr(closure, 1) as *mut ArrayHeader;
    let state_arr = js_closure_get_capture_ptr(closure, 2) as *mut ArrayHeader;
    if result_promise.is_null() || results_arr.is_null() || state_arr.is_null() {
        return 0.0;
    }
    let index = js_closure_get_capture_f64(closure, 3) as u32;

    let wrapped = build_settled_rejected(reason);
    js_array_set_f64(results_arr, index, wrapped);

    let remaining = js_array_get_f64(state_arr, 0) - 1.0;
    js_array_set_f64(state_arr, 0, remaining);

    if remaining == 0.0 {
        let arr_f64 = crate::value::js_nanbox_pointer(results_arr as i64);
        js_promise_resolve(result_promise, arr_f64);
    }
    0.0
}

/// Promise.any — settles with the first FULFILLED promise. If all reject, rejects
/// with an `AggregateError` whose `errors` array carries the collected reasons
/// (constructed via `js_aggregate_error_new` in the all-rejected path below).
#[no_mangle]
pub extern "C" fn js_promise_any(promises_arr: *const crate::array::ArrayHeader) -> *mut Promise {
    use crate::array::{js_array_alloc, js_array_get_f64, js_array_length, js_array_set_f64};
    use crate::closure::{
        js_closure_alloc, js_closure_set_capture_f64, js_closure_set_capture_ptr,
    };
    use crate::value::js_nanbox_get_pointer;

    let result_promise = js_promise_new();

    if promises_arr.is_null() {
        // Empty input — Promise.any rejects immediately with empty errors array
        let errors_arr = js_array_alloc(0);
        unsafe {
            (*errors_arr).length = 0;
        }
        let arr_f64 = crate::value::js_nanbox_pointer(errors_arr as i64);
        js_promise_reject(result_promise, arr_f64);
        return result_promise;
    }

    let count = js_array_length(promises_arr);
    if count == 0 {
        let errors_arr = js_array_alloc(0);
        unsafe {
            (*errors_arr).length = 0;
        }
        let arr_f64 = crate::value::js_nanbox_pointer(errors_arr as i64);
        js_promise_reject(result_promise, arr_f64);
        return result_promise;
    }

    let errors_arr = js_array_alloc(count);
    unsafe {
        (*errors_arr).length = count;
    }
    const TAG_UNDEFINED: u64 = 0x7FFC_0000_0000_0001;
    for i in 0..count {
        js_array_set_f64(errors_arr, i, f64::from_bits(TAG_UNDEFINED));
    }

    // state: [remaining_rejections, settled_flag]
    let state_arr = js_array_alloc(2);
    unsafe {
        (*state_arr).length = 2;
    }
    js_array_set_f64(state_arr, 0, count as f64);
    js_array_set_f64(state_arr, 1, 0.0);

    for i in 0..count {
        let promise_f64 = js_array_get_f64(promises_arr, i);
        // Discriminate via GC-header obj_type — string/bigint NaN-boxed
        // values would otherwise pass through pointer extraction and crash
        // js_promise_then.
        if js_value_is_promise(promise_f64) == 0 {
            // Non-promise value — treat as fulfilled, settle immediately if not yet settled
            let already_settled = js_array_get_f64(state_arr, 1);
            if already_settled == 0.0 {
                js_array_set_f64(state_arr, 1, 1.0);
                js_promise_resolve(result_promise, promise_f64);
            }
            return result_promise;
        }
        let promise_ptr = js_nanbox_get_pointer(promise_f64) as *mut Promise;

        let fulfill_closure = js_closure_alloc(promise_any_fulfill_handler as *const u8, 2);
        js_closure_set_capture_ptr(fulfill_closure, 0, result_promise as i64);
        js_closure_set_capture_ptr(fulfill_closure, 1, state_arr as i64);

        let reject_closure = js_closure_alloc(promise_any_reject_handler as *const u8, 4);
        js_closure_set_capture_ptr(reject_closure, 0, result_promise as i64);
        js_closure_set_capture_ptr(reject_closure, 1, errors_arr as i64);
        js_closure_set_capture_ptr(reject_closure, 2, state_arr as i64);
        js_closure_set_capture_f64(reject_closure, 3, i as f64);

        js_promise_attach_handlers(promise_ptr, fulfill_closure, reject_closure);
    }

    result_promise
}

extern "C" fn promise_any_fulfill_handler(
    closure: *const crate::closure::ClosureHeader,
    value: f64,
) -> f64 {
    use crate::array::{js_array_get_f64, js_array_set_f64, ArrayHeader};
    use crate::closure::js_closure_get_capture_ptr;

    let result_promise = js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    let state_arr = js_closure_get_capture_ptr(closure, 1) as *mut ArrayHeader;
    if result_promise.is_null() || state_arr.is_null() {
        return 0.0;
    }

    let already_settled = js_array_get_f64(state_arr, 1);
    if already_settled != 0.0 {
        return 0.0;
    }
    js_array_set_f64(state_arr, 1, 1.0);

    js_promise_resolve(result_promise, value);
    0.0
}

extern "C" fn promise_any_reject_handler(
    closure: *const crate::closure::ClosureHeader,
    reason: f64,
) -> f64 {
    use crate::array::{js_array_get_f64, js_array_set_f64, ArrayHeader};
    use crate::closure::{js_closure_get_capture_f64, js_closure_get_capture_ptr};

    let result_promise = js_closure_get_capture_ptr(closure, 0) as *mut Promise;
    let errors_arr = js_closure_get_capture_ptr(closure, 1) as *mut ArrayHeader;
    let state_arr = js_closure_get_capture_ptr(closure, 2) as *mut ArrayHeader;
    if result_promise.is_null() || errors_arr.is_null() || state_arr.is_null() {
        return 0.0;
    }
    let index = js_closure_get_capture_f64(closure, 3) as u32;

    let already_settled = js_array_get_f64(state_arr, 1);
    if already_settled != 0.0 {
        return 0.0;
    }

    js_array_set_f64(errors_arr, index, reason);

    let remaining = js_array_get_f64(state_arr, 0) - 1.0;
    js_array_set_f64(state_arr, 0, remaining);

    if remaining == 0.0 {
        // All rejected — create an AggregateError with the collected
        // errors array and reject the result promise with it.
        js_array_set_f64(state_arr, 1, 1.0);
        let msg = crate::string::js_string_from_bytes(b"All promises were rejected".as_ptr(), 26);
        let agg_err = crate::error::js_aggregateerror_new(errors_arr, msg);
        let err_f64 = crate::value::js_nanbox_pointer(agg_err as i64);
        js_promise_reject(result_promise, err_f64);
    }
    0.0
}

/// GC root scanner: mark all values reachable from promise task queues
pub fn scan_promise_roots(mark: &mut dyn FnMut(f64)) {
    // Scan TASK_QUEUE entries
    TASK_QUEUE.with(|q| {
        let q = q.borrow();
        for entry in q.iter() {
            match entry {
                Task::Promise(promise_ptr, value, _) => {
                    if !promise_ptr.is_null() {
                        let boxed = f64::from_bits(
                            0x7FFD_0000_0000_0000
                                | (*promise_ptr as u64 & 0x0000_FFFF_FFFF_FFFF),
                        );
                        mark(boxed);
                    }
                    mark(*value);
                }
                Task::Inline(cb, value, next, _) => {
                    if !cb.is_null() {
                        let boxed = f64::from_bits(
                            0x7FFD_0000_0000_0000 | (*cb as u64 & 0x0000_FFFF_FFFF_FFFF),
                        );
                        mark(boxed);
                    }
                    if !next.is_null() {
                        let boxed = f64::from_bits(
                            0x7FFD_0000_0000_0000
                                | (*next as u64 & 0x0000_FFFF_FFFF_FFFF),
                        );
                        mark(boxed);
                    }
                    mark(*value);
                }
                Task::AsyncStep(cb, value, next, _) => {
                    if !cb.is_null() {
                        let boxed = f64::from_bits(
                            0x7FFD_0000_0000_0000 | (*cb as u64 & 0x0000_FFFF_FFFF_FFFF),
                        );
                        mark(boxed);
                    }
                    if !next.is_null() {
                        let boxed = f64::from_bits(
                            0x7FFD_0000_0000_0000
                                | (*next as u64 & 0x0000_FFFF_FFFF_FFFF),
                        );
                        mark(boxed);
                    }
                    mark(*value);
                }
            }
        }
    });

    // Scan SCHEDULED_RESOLVES entries
    SCHEDULED_RESOLVES.with(|q| {
        let q = q.borrow();
        for &(promise_ptr, value) in q.iter() {
            if !promise_ptr.is_null() {
                let boxed = f64::from_bits(
                    0x7FFD_0000_0000_0000 | (promise_ptr as u64 & 0x0000_FFFF_FFFF_FFFF),
                );
                mark(boxed);
            }
            mark(value);
        }
    });
}

/// Promise.withResolvers<T>() — returns an object with { promise, resolve, reject }.
/// The resolve/reject are closures that settle the promise when called.
#[no_mangle]
pub extern "C" fn js_promise_with_resolvers() -> *mut crate::object::ObjectHeader {
    use crate::closure::js_closure_alloc;
    use crate::object::{js_object_alloc_with_shape, ObjectHeader};

    // Create the pending promise.
    let promise = js_promise_new();
    let promise_box = crate::value::js_nanbox_pointer(promise as i64);

    // Create resolve closure that resolves this promise.
    let resolve_fn = js_closure_alloc(
        with_resolvers_resolve_handler as *const u8,
        1, // 1 capture: the promise pointer
    );
    unsafe {
        crate::closure::js_closure_set_capture_f64(resolve_fn, 0, promise_box);
    }
    let resolve_box = crate::value::js_nanbox_pointer(resolve_fn as i64);

    // Create reject closure.
    let reject_fn = js_closure_alloc(with_resolvers_reject_handler as *const u8, 1);
    unsafe {
        crate::closure::js_closure_set_capture_f64(reject_fn, 0, promise_box);
    }
    let reject_box = crate::value::js_nanbox_pointer(reject_fn as i64);

    // Build the { promise, resolve, reject } object.
    // Use a 3-field object with packed keys "promise\0resolve\0reject\0".
    let packed = b"promise\0resolve\0reject\0";
    let obj = js_object_alloc_with_shape(
        0xFFF0_0001, // unique shape id
        3,
        packed.as_ptr(),
        packed.len() as u32,
    );

    // Store the three fields.
    unsafe {
        let fields = (obj as *mut u8).add(std::mem::size_of::<ObjectHeader>()) as *mut f64;
        *fields.add(0) = promise_box; // .promise
        *fields.add(1) = resolve_box; // .resolve
        *fields.add(2) = reject_box; // .reject
    }

    obj
}

extern "C" fn with_resolvers_resolve_handler(
    closure: *const crate::closure::ClosureHeader,
    value: f64,
) -> f64 {
    unsafe {
        let promise_box = crate::closure::js_closure_get_capture_f64(closure, 0);
        let promise_ptr = (f64::to_bits(promise_box) & crate::value::POINTER_MASK) as *mut Promise;
        js_promise_resolve(promise_ptr, value);
    }
    f64::from_bits(crate::value::TAG_UNDEFINED)
}

extern "C" fn with_resolvers_reject_handler(
    closure: *const crate::closure::ClosureHeader,
    value: f64,
) -> f64 {
    unsafe {
        let promise_box = crate::closure::js_closure_get_capture_f64(closure, 0);
        let promise_ptr = (f64::to_bits(promise_box) & crate::value::POINTER_MASK) as *mut Promise;
        js_promise_reject(promise_ptr, value);
    }
    f64::from_bits(crate::value::TAG_UNDEFINED)
}

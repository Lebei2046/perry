# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

**NOTE**: Keep this file concise. Detailed changelogs live in CHANGELOG.md.

## Project Overview

Perry is a native TypeScript compiler written in Rust that compiles TypeScript source code directly to native executables. It uses SWC for TypeScript parsing and LLVM for code generation.

**Current Version:** 0.5.404

## TypeScript Parity Status

Tracked via the gap test suite (`test-files/test_gap_*.ts`, 28 tests). Compared byte-for-byte against `node --experimental-strip-types`. Run via `/tmp/run_gap_tests.sh` after `cargo build --release -p perry-runtime -p perry-stdlib -p perry`.

**Last sweep (v0.5.388):** **27/28 passing.** `typed_arrays` flipped to pass after the b2b0a3e6 Uint8ClampedArray work. Only `console_methods` still fails — and only on the macos-14 CI runner; passes locally. Long-documented `ci-env` quirk (`console.time` value normalization edge case). Parity sweep: **167/167 (100%)** with both v0.5.388 fixes landed for #302 (Map/Set on class field) + #154 (using/dispose SIGBUS). Run via `/tmp/run_gap_tests.sh` and `./run_parity_tests.sh` after full rebuild.

**Known categorical gaps**: lookbehind regex (Rust `regex` crate), `console.dir`/`console.group*` formatting, lone surrogate handling (WTF-8).

## Workflow Requirements

**IMPORTANT:** Follow these practices for every code change made directly on `main` (maintainer workflow):

1. **Update CLAUDE.md**: Add 1-2 line entry in "Recent Changes" for new features/fixes
2. **Increment Version**: Bump patch version (e.g., 0.5.48 → 0.5.49)
3. **Commit Changes**: Include code changes and CLAUDE.md updates together

### External contributor PRs

PRs from outside contributors should **not** touch `[workspace.package] version` in `Cargo.toml`, the `**Current Version:**` line in `CLAUDE.md`, or add a "Recent Changes" entry. The maintainer bumps the version and writes the changelog entry at merge time — usually by rebasing the PR branch and amending. This avoids the patch-version collisions that happen when Perry's `main` ships several commits while a PR is in review (each on-main commit bumps the version; a PR that bumped to the same patch on day 1 is already behind by merge day). Contributors just write code; let the maintainer fold in the metadata last.

## Build Commands

```bash
cargo build --release                          # Build all crates
cargo build --release -p perry-runtime -p perry-stdlib  # Rebuild runtime (MUST rebuild stdlib too!)
cargo test --workspace --exclude perry-ui-ios  # Run tests (exclude iOS on macOS host)
cargo run --release -- file.ts -o output && ./output    # Compile and run TypeScript
cargo run --release -- file.ts --print-hir              # Debug: print HIR
```

## Architecture

```
TypeScript (.ts) → Parse (SWC) → AST → Lower → HIR → Transform → Codegen (LLVM) → .o → Link (cc) → Executable
```

| Crate | Purpose |
|-------|---------|
| **perry** | CLI driver (parallel module codegen via rayon) |
| **perry-parser** | SWC wrapper for TypeScript parsing |
| **perry-types** | Type system definitions |
| **perry-hir** | HIR data structures (`ir.rs`) and AST→HIR lowering (`lower.rs`) |
| **perry-transform** | IR passes (closure conversion, async lowering, inlining) |
| **perry-codegen** | LLVM-based native code generation |
| **perry-runtime** | Runtime: value.rs, object.rs, array.rs, string.rs, gc.rs, arena.rs, thread.rs |
| **perry-stdlib** | Node.js API support (mysql2, redis, fetch, fastify, ws, etc.) |
| **perry-ui** / **perry-ui-macos** / **perry-ui-ios** / **perry-ui-tvos** | Native UI (AppKit/UIKit) |
| **perry-jsruntime** | JavaScript interop via QuickJS |

## NaN-Boxing

Perry uses NaN-boxing to represent JavaScript values in 64 bits (`perry-runtime/src/value.rs`):

```
TAG_UNDEFINED = 0x7FFC_0000_0000_0001    BIGINT_TAG  = 0x7FFA (lower 48 = ptr)
TAG_NULL      = 0x7FFC_0000_0000_0002    POINTER_TAG = 0x7FFD (lower 48 = ptr)
TAG_FALSE     = 0x7FFC_0000_0000_0003    INT32_TAG   = 0x7FFE (lower 32 = int)
TAG_TRUE      = 0x7FFC_0000_0000_0004    STRING_TAG  = 0x7FFF (lower 48 = ptr)
```

Key functions: `js_nanbox_string/pointer/bigint`, `js_nanbox_get_pointer`, `js_get_string_pointer_unified`, `js_jsvalue_to_string`, `js_is_truthy`

**Module-level variables**: Strings stored as F64 (NaN-boxed), Arrays/Objects as I64 (raw pointers). Access via `module_var_data_ids`.

## Garbage Collection

Generational mark-sweep GC in `crates/perry-runtime/src/gc.rs` (default since v0.5.237 / Phase D). Two regions in the per-thread arena: nursery (`ARENA`, fills with new allocations, swept on minor GC) and old-gen (`OLD_ARENA`, holds tenured/evacuated objects). Conservative stack scan + precise shadow-stack roots + 9 registered scanners. Write barriers populate a remembered set so minor GC can avoid retracing the old-gen. Two-bit aging (`HAS_SURVIVED` / `TENURED`) promotes nursery survivors after 2 minor cycles; the C4b evacuation pass moves non-pinned tenured objects into old-gen with full reference rewriting. Idle nursery blocks observed empty for 2 GC cycles are `dealloc`'d back to the OS (C4b-δ, v0.5.235), and the next-trigger calc is hard-capped at the initial threshold (64 MB) so >90%-freed step-doubling can't blow up peak occupancy (C4b-δ-tune, v0.5.236). Triggers on arena block allocation (1 MB blocks since v0.5.196), malloc count threshold, or explicit `gc()` call. 8-byte GcHeader per allocation.

**Escape hatches**: `PERRY_GEN_GC=0`/`off`/`false` reverts to full mark-sweep (bisection only). `PERRY_GEN_GC_EVACUATE=1` enables the copying evacuation pass (default OFF — complete and correctness-safe but adds work that's a no-op on workloads where nothing tenures). `PERRY_WRITE_BARRIERS=1` opts into codegen-emitted write barriers (default OFF — barrier emission has its own perf cost; the runtime barrier always exists). `PERRY_GC_DIAG=1` prints per-cycle diagnostics.

## Threading (`perry/thread`)

Single-threaded by default. `perry/thread` provides:
- **`parallelMap(array, fn)`** / **`parallelFilter(array, fn)`** — data-parallel across all cores
- **`spawn(fn)`** — background OS thread, returns Promise

Values cross threads via `SerializedValue` deep-copy. Each thread has independent arena + GC. Results from `spawn` flow back via `PENDING_THREAD_RESULTS` queue, drained during `js_promise_run_microtasks()`.

## Native UI (`perry/ui`)

Declarative TypeScript compiles to AppKit/UIKit calls. Handle-based widget system (1-based i64 handles, NaN-boxed with POINTER_TAG). `--target ios-simulator`/`--target ios`/`--target tvos-simulator`/`--target tvos` for cross-compilation.

**To add a new widget** — change 4 places:
1. Runtime: `crates/perry-ui-macos/src/widgets/` — create widget, `register_widget(view)`
2. FFI: `crates/perry-ui-macos/src/lib.rs` — `#[no_mangle] pub extern "C" fn perry_ui_<widget>_create`
3. Codegen: `crates/perry-codegen/src/codegen.rs` — declare extern + NativeMethodCall dispatch
4. HIR: `crates/perry-hir/src/lower.rs` — only if widget has instance methods

## Compiling npm Packages Natively (`perry.compilePackages`)

Configured in `package.json`:
```json
{ "perry": { "compilePackages": ["@noble/curves", "@noble/hashes"] } }
```
First-resolved directory cached in `compile_package_dirs`; subsequent imports redirect to the same copy (dedup).

## Known Limitations

- **No runtime type checking**: Types erased at compile time. `typeof` via NaN-boxing tags. `instanceof` via class ID chain.
- **No shared mutable state across threads**: No `SharedArrayBuffer` or `Atomics`.

## Common Pitfalls & Patterns

### NaN-Boxing Mistakes
- **Double NaN-boxing**: If value is already F64, don't NaN-box again. Check `builder.func.dfg.value_type(val)`.
- **Wrong tag**: Strings=STRING_TAG, objects=POINTER_TAG, BigInt=BIGINT_TAG.
- **`as f64` vs `from_bits`**: `u64 as f64` is numeric conversion (WRONG). Use `f64::from_bits(u64)` to preserve bits.

### LLVM Type Mismatches
- Loop counter optimization produces i32 — always convert before passing to f64/i64 functions
- Constructor parameters always f64 (NaN-boxed) at signature level

### Async / Threading
- Thread-local arenas: JSValues from tokio workers invalid on main thread
- Use `spawn_for_promise_deferred()` — return raw Rust data, convert to JSValue on main thread
- Async closures: Promise pointer (I64) must be NaN-boxed with POINTER_TAG before returning as F64

### Cross-Module Issues
- ExternFuncRef values are NaN-boxed — use `js_nanbox_get_pointer` to extract
- Module init order: topological sort by import dependencies
- Optional params need `imported_func_param_counts` propagation through re-exports

### Closure Captures
- `collect_local_refs_expr()` must handle all expression types — catch-all silently skips refs
- Captured string/pointer values must be NaN-boxed before storing, not raw bitcast
- Loop counter i32 values: `fcvt_from_sint` to f64 before capture storage

### Handle-Based Dispatch
- TWO systems: `HANDLE_METHOD_DISPATCH` (methods) and `HANDLE_PROPERTY_DISPATCH` (properties)
- Both must be registered. Small pointer detection: value < 0x100000 = handle.

### objc2 v0.6 API
- `define_class!` with `#[unsafe(super(NSObject))]`, `msg_send!` returns `Retained` directly
- All AppKit constructors require `MainThreadMarker`

## Recent Changes

Keep entries to 1-2 lines max. Full details in CHANGELOG.md.

- **v0.5.404** — Closes #242: visionOS now registers the full geisterhand fn-pointer block (state_set / screenshot / textfield / scroll / read_value / query_tree / apply_style) — Phase D is fully cross-platform on every Apple target Perry supports (macOS / iOS / tvOS / visionOS).
- **v0.5.403** — Closes #309: `perry compile` of a 4-line program importing the `effect` package OOM'd at **34 GB RSS / 249 GB peak virtual** before being SIGKILL'd ~7 minutes in, during the `Generating code...` phase.
- **v0.5.402** — Closes #311: `for...of` on a Map/Set held as a property of a plain object literal OR a class instance silently iterated zero times.
- **v0.5.401** — HarmonyOS Phase 2 v1.5: full widget set in `perry-codegen-arkts`.
- **v0.5.400** — Closes #303: opt-in Win7 / Win8 / 8.1 compatibility for compiled executables via new `--min-windows-version=7|8|10` CLI flag (default `10`).
- **v0.5.399** — HarmonyOS Phase 2 v1: TS→ArkUI emission via new `perry-codegen-arkts` crate.
- **v0.5.398** — Closes #307: `JSON.stringify(parseResult)` returned the literal string `"null"` for objects with **≥9 fields**, silently corrupting any program that round-trips JSON (perry-hub built every CI build's `job_assign` manifest as `null` for two days before the user pinned it down).
- **v0.5.397** — Closes #304 + #305 (two small wins from real-world repros).
- **v0.5.396** — Closes #245 Phase 2: workspace-wide `cargo clippy --fix` auto-correction sweep.
- **v0.5.395** — Closes the loop on #229: sign-side CLI tool for v2 manifest signatures.
- **v0.5.394** — v0.5.393 cargo-test follow-up: also exclude `perry-jsruntime` from `cargo test --workspace` on ubuntu-latest.
- **v0.5.393** — Release-night reliability + v0.5.392 follow-up.
- **v0.5.392** — CI cost reduction: move `cargo-test`, `parity`, and `compile-smoke` jobs from `macos-14` (10× billing weight) → `ubuntu-latest` (1×) in `.github/workflows/test.yml`.
- **v0.5.391** — Closes #229: version-binding into the perry-updater signed payload (Option A from the design comment).
- **v0.5.390** — Closes #228: enforce HTTPS on `@perry/updater`'s manifest + asset URLs.
- **v0.5.389** — Closes #300 (Windows SDK auto-discovery) + cosmetic cleanup for #266.
- **v0.5.388** — Two real bug fixes that flipped the last 2 entries from `known_failures.json` to passing — Perry's parity rate is now 100% (167/167) and gap tests are 27/28 (only `console_methods` remains, a long-documented ci-env quirk).
- **v0.5.387** — CI overhaul (post-mortem from v0.5.386's painful release night).
- **v0.5.386** — Hotfix: v0.5.385's new HIR arm for `module.Class.staticMethod()` over-fired and broke `fs.promises.readFile()` (and likely `fs.promises.writeFile/mkdir/access/...` + `fs.constants.X` + `path.posix.X` + `path.win32.X`).
- **v0.5.385** — Closes #278 (PR #299, manually rebased + landed): wire codegen dispatch for the 3 perry/system + ethers holdouts that #278 documented.
- **v0.5.384** — CI hotfix-of-hotfix: with v0.5.383's nullglob fix actually landed, the compile-smoke run on 0bdccbd4 successfully printed `Compile smoke: 183 passed, 1 failed, 4 skipped` — and the lone failure surfaced a **separate race condition I introduced via parallelization** (NJOBS=6 was…
- **v0.5.383** — CI hotfix: my v0.5.381 nullglob fix for the compile-smoke errexit bug **never actually landed in 72d17ff8** — the diff hunk that replaced `PASS=$(ls -1 *.pass | wc -l)` with `shopt -s nullglob; pass_files=("$LOGS_DIR"/*.pass); PASS=${#pass_files[@]}` got separated from the…
- **v0.5.382** — Closes #291: three independent SIGSEGV bug shapes from the same issue, fixed across 4 files.

Older entries → CHANGELOG.md.

//! Standard Library for Perry
//!
//! Feature-gated implementations of Node.js APIs and npm packages.
//! Only compile what you actually use for smaller binaries.
//!
//! # Features
//! - `core` - Minimal runtime (always included)
//! - `http-server` - Native HTTP server (hyper-based)
//! - `http-client` - HTTP client (reqwest/node-fetch)
//! - `database` - All databases (postgres, mysql, sqlite, redis, mongodb)
//! - `crypto` - Cryptographic functions
//! - `compression` - zlib compression
//! - `full` - Everything (default)

// Re-export the updater crate so its #[no_mangle] FFI symbols are
// retained in libperry_stdlib.a (Cargo would otherwise drop unused
// rlib deps during the staticlib bundle step).
pub use perry_updater;

// `extern "C"` shims that perry-ffi declares for use by external
// native binding crates (#466 Phase 1 + 5 — async surface). Gated
// on `async-runtime` because the underlying async_bridge does;
// every wrapper that depends on these (bcrypt, argon2, ws, db
// drivers, …) already triggers `async-runtime` through its own
// per-binding feature, so the linkage is automatic.
#[cfg(feature = "async-runtime")]
pub mod perry_ffi_async;

// Core modules - always available
pub mod async_local_storage;
pub mod commander;
pub mod common;
pub mod dayjs;
pub mod decimal;
// dotenv is feature-gated as of v0.5.533 so the well-known bindings
// table (#466 Phase 4) can route `import 'dotenv'` to perry-ext-dotenv
// without duplicate _js_dotenv_* symbols at link time. Default-on
// preserves byte-identical behavior for programs that don't opt into
// the well-known path.
#[cfg(feature = "bundled-dotenv")]
pub mod dotenv;
pub mod events;
// exponential_backoff feature-gated as of v0.5.542 so the
// well-known flip can route to perry-ext-exponential-backoff.
#[cfg(feature = "bundled-exponential-backoff")]
pub mod exponential_backoff;
pub mod lodash;
// lru_cache is feature-gated as of v0.5.539 so the well-known
// flip can route `import 'lru-cache'` to perry-ext-lru-cache.
#[cfg(feature = "bundled-lru-cache")]
pub mod lru_cache;
pub mod moment;
pub mod readline;
// slugify is feature-gated as of v0.5.536 so the well-known bindings
// flip can route `import 'slugify'` to perry-ext-slugify cleanly.
// Default-on through `default = ["full"]`.
#[cfg(feature = "bundled-slugify")]
pub mod slugify;
pub mod worker_threads;

// Re-export core
pub use async_local_storage::*;
pub use commander::*;
pub use common::*;
pub use dayjs::*;
pub use decimal::*;
#[cfg(feature = "bundled-dotenv")]
pub use dotenv::*;
pub use events::*;
#[cfg(feature = "bundled-exponential-backoff")]
pub use exponential_backoff::*;
pub use lodash::*;
#[cfg(feature = "bundled-lru-cache")]
pub use lru_cache::*;
pub use moment::*;
pub use readline::*;
#[cfg(feature = "bundled-slugify")]
pub use slugify::*;
pub use worker_threads::*;

// === HTTP Server ===
#[cfg(feature = "http-server")]
pub mod framework;
#[cfg(feature = "http-server")]
pub use framework::*;

// === Fastify-Compatible Framework ===
#[cfg(feature = "http-server")]
pub mod fastify;
#[cfg(feature = "http-server")]
pub use fastify::*;

// === HTTP Client ===
#[cfg(feature = "http-client")]
pub mod fetch;
#[cfg(feature = "http-client")]
pub use fetch::*;

#[cfg(feature = "http-client")]
pub mod http;
#[cfg(feature = "http-client")]
pub use http::*;

#[cfg(feature = "http-client")]
pub mod axios;
#[cfg(feature = "http-client")]
pub use axios::*;

// === Web Streams API (issue #237) ===
#[cfg(feature = "http-client")]
pub mod streams;
#[cfg(feature = "http-client")]
pub use streams::*;

// === WebSocket ===
#[cfg(feature = "websocket")]
pub mod ws;
#[cfg(feature = "websocket")]
pub use ws::*;

// === Raw TCP sockets (net.Socket) + TLS (tls.connect, socket.upgradeToTLS) ===
// Desktop only; iOS/Android stdlib are stubs for now.
#[cfg(all(feature = "net", not(target_os = "ios"), not(target_os = "android")))]
pub mod net;
#[cfg(all(feature = "net", not(target_os = "ios"), not(target_os = "android")))]
pub use net::*;

// === Databases ===
#[cfg(any(feature = "database-postgres", feature = "database-mysql"))]
pub mod pg;
#[cfg(any(feature = "database-postgres", feature = "database-mysql"))]
pub use pg::connection::*;
#[cfg(any(feature = "database-postgres", feature = "database-mysql"))]
pub use pg::pool::*;

#[cfg(any(feature = "database-postgres", feature = "database-mysql"))]
pub mod mysql2;
#[cfg(any(feature = "database-postgres", feature = "database-mysql"))]
pub use mysql2::connection::*;
#[cfg(any(feature = "database-postgres", feature = "database-mysql"))]
pub use mysql2::pool::*;

#[cfg(feature = "database-sqlite")]
pub mod sqlite;
#[cfg(feature = "database-sqlite")]
pub use sqlite::*;

#[cfg(feature = "database-redis")]
pub mod ioredis;
#[cfg(feature = "database-redis")]
pub use ioredis::*;

#[cfg(feature = "database-mongodb")]
pub mod mongodb;
#[cfg(feature = "database-mongodb")]
pub use mongodb::*;

// === Crypto ===
#[cfg(feature = "crypto")]
pub mod crypto;
#[cfg(feature = "crypto")]
pub use crypto::*;

// === Ethers (blockchain utilities) ===
#[cfg(feature = "crypto")]
pub mod ethers;
#[cfg(feature = "crypto")]
pub use ethers::*;

// bcrypt + argon2 split out from the broad `crypto` feature in
// v0.5.537 so the well-known flip can swap each one out
// individually. The `crypto` umbrella still pulls them both in
// (`crypto = [..., "bundled-bcrypt", "bundled-argon2"]`) so legacy
// `--features crypto` builds keep producing byte-identical archives.
#[cfg(feature = "bundled-bcrypt")]
pub mod bcrypt;
#[cfg(feature = "bundled-bcrypt")]
pub use bcrypt::*;

#[cfg(feature = "bundled-argon2")]
pub mod argon2;
#[cfg(feature = "bundled-argon2")]
pub use argon2::*;

// jsonwebtoken split out into `bundled-jsonwebtoken` (v0.5.538)
// for the same reason as bcrypt/argon2 — well-known flip
// independence. The `crypto` umbrella still pulls it in for
// backwards compat.
#[cfg(feature = "bundled-jsonwebtoken")]
pub mod jsonwebtoken;
#[cfg(feature = "bundled-jsonwebtoken")]
pub use jsonwebtoken::*;

#[cfg(feature = "crypto")]
pub mod crypto_e2e;
#[cfg(feature = "crypto")]
pub use crypto_e2e::*;

// === Compression ===
#[cfg(feature = "compression")]
pub mod zlib;
#[cfg(feature = "compression")]
pub use zlib::*;

// === Email ===
#[cfg(feature = "email")]
pub mod nodemailer;
#[cfg(feature = "email")]
pub use nodemailer::*;

// === Image Processing ===
#[cfg(feature = "image")]
pub mod sharp;
#[cfg(feature = "image")]
pub use sharp::*;

// === HTML Parsing ===
#[cfg(feature = "html-parser")]
pub mod cheerio;
#[cfg(feature = "html-parser")]
pub use cheerio::*;

// === Scheduler ===
#[cfg(feature = "scheduler")]
pub mod cron;
#[cfg(feature = "scheduler")]
pub use cron::*;

// Unconditional cron timer stubs — always present so the CLI event loop in
// `module_init.rs` can call `js_cron_timer_tick` / `js_cron_timer_has_pending`
// even when the `scheduler` feature is disabled (e.g. an auto-optimized build
// of a project that imports `node:crypto` but not `node-cron`). With the
// scheduler feature ENABLED, these symbols are provided by `cron.rs` instead;
// the `#[cfg(not(feature = "scheduler"))]` gate below prevents a duplicate
// symbol error in that case.
#[cfg(not(feature = "scheduler"))]
#[no_mangle]
pub extern "C" fn js_cron_timer_tick() -> i32 {
    0
}
#[cfg(not(feature = "scheduler"))]
#[no_mangle]
pub extern "C" fn js_cron_timer_has_pending() -> i32 {
    0
}

// === Rate Limiting ===
#[cfg(feature = "rate-limit")]
pub mod ratelimit;
#[cfg(feature = "rate-limit")]
pub use ratelimit::*;

// === Validation ===
// `validation` umbrella now expands to `bundled-validator`
// (v0.5.538). Per-binding gate lets the well-known flip swap the
// validator wrapper out without affecting the rest of the
// validation surface (none — there's just the one wrapper today,
// but the split unblocks future additions).
#[cfg(feature = "bundled-validator")]
pub mod validator;
#[cfg(feature = "bundled-validator")]
pub use validator::*;

// === IDs ===
// `bundled-uuid` / `bundled-nanoid` (v0.5.534) replace the old
// `ids` umbrella so the well-known flip (#466 Phase 4) can toggle
// each binding independently. The umbrella stays as
// `ids = ["bundled-uuid", "bundled-nanoid"]` so existing
// `--features ids` callers keep working byte-identically.
#[cfg(feature = "bundled-uuid")]
pub mod uuid;
#[cfg(feature = "bundled-uuid")]
pub use uuid::*;

#[cfg(feature = "bundled-nanoid")]
pub mod nanoid;
#[cfg(feature = "bundled-nanoid")]
pub use nanoid::*;

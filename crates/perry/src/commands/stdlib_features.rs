//! Map TypeScript native module imports → perry-stdlib Cargo features.
//!
//! Used by `--minimal-stdlib` to compute the minimal feature set needed
//! for a project, then rebuild perry-stdlib with `--no-default-features
//! --features <list>` so the linker only sees the subsystems the project
//! actually uses.
//!
//! Modules handled by perry-runtime alone (fs, path, os, buffer, perry/ui,
//! perry/i18n, etc. — see `perry_hir::requires_stdlib`) are skipped here
//! because they don't trigger stdlib linkage at all.

use std::collections::BTreeSet;

/// Look up the perry-stdlib feature(s) required to support a single
/// imported native module. Returns an empty slice for modules that need
/// no optional stdlib feature (covered by always-on dependencies like
/// chrono / lru / decimal).
pub fn module_to_features(module: &str) -> &'static [&'static str] {
    let normalized = module.strip_prefix("node:").unwrap_or(module);
    match normalized {
        // ── HTTP server (Hyper) ───────────────────────────────────────
        "fastify" => &["http-server"],

        // ── HTTP client (reqwest) ─────────────────────────────────────
        "axios" | "node-fetch" => &["http-client"],

        // ── WebSocket ─────────────────────────────────────────────────
        "ws" => &["websocket"],

        // ── Raw TCP sockets (net.Socket) ──────────────────────────────
        // `upgradeToTLS` is a method on net.Socket, so any program using
        // `net` must link the TLS runtime too — otherwise `sock.upgradeToTLS`
        // fails at link time with `_js_net_socket_upgrade_tls` undefined.
        // The binary-size cost is small; programs that explicitly want
        // zero TLS bytes can still opt in via the lower-level feature flags.
        "net" => &["net", "tls"],

        // ── TLS (tls.connect, socket.upgradeToTLS) ───────────────────
        "tls" => &["tls"],

        // ── Databases ─────────────────────────────────────────────────
        "mysql2" | "mysql2/promise" => &["database-mysql"],
        "pg" => &["database-postgres"],
        "better-sqlite3" => &["database-sqlite"],
        // Redis is detected via the ioredis class name in collect_modules,
        // but if it shows up as an explicit import we still need the feature.
        "ioredis" | "redis" => &["database-redis"],
        "mongodb" => &["database-mongodb"],

        // ── Crypto ────────────────────────────────────────────────────
        // bcrypt split off into its own `bundled-bcrypt` feature in
        // v0.5.537 so the well-known flip can route it to
        // perry-ext-bcrypt without taking the rest of the crypto
        // surface offline. The `crypto` umbrella still includes
        // bundled-bcrypt for backwards compat — programs that import
        // bcrypt also typically use sha256/jwt/etc., which keeps the
        // umbrella worthwhile.
        "bcrypt" => &["bundled-bcrypt"],
        "jsonwebtoken" => &["bundled-jsonwebtoken"],
        "crypto" => &["crypto"],
        // ethers ships utility functions (formatUnits, parseUnits,
        // getAddress, keccak256, …) that bottom out in sha3/keccak in
        // the crypto bucket.
        "ethers" => &["crypto"],
        // perry/updater's signature verification routes through
        // js_crypto_ed25519_verify in perry-stdlib::crypto, so importing
        // perry/updater pulls in the crypto feature transitively.
        "perry/updater" => &["crypto"],

        // ── Compression (zlib) ────────────────────────────────────────
        "zlib" => &["compression"],

        // ── Email (lettre) ────────────────────────────────────────────
        "nodemailer" => &["email"],

        // ── Image processing (sharp) ──────────────────────────────────
        "sharp" => &["image"],

        // ── HTML parsing (cheerio / scraper) ──────────────────────────
        "cheerio" => &["html-parser"],

        // ── Scheduler (cron) ──────────────────────────────────────────
        "cron" | "node-cron" => &["scheduler"],

        // ── Validation (validator.js) ─────────────────────────────────
        // `validation` umbrella retained for backwards-compat;
        // per-binding gate is `bundled-validator` (v0.5.538).
        "validator" => &["bundled-validator"],

        // ── argon2 ────────────────────────────────────────────────────
        // argon2 split off into `bundled-argon2` (v0.5.537) — same
        // reason as bcrypt above. Note: NATIVE_MODULES doesn't list
        // `argon2` in v0.5.532's manifest because perry-stdlib's
        // existing dispatch routes it through a different code path,
        // but the feature mapping is still useful for future parity.
        "argon2" => &["bundled-argon2"],

        // ── IDs (uuid / nanoid) ───────────────────────────────────────
        // Per-binding split as of v0.5.534 (#466 Phase 4 step 2)
        // so the well-known flip can swap each one out
        // independently. The `ids` umbrella stays in
        // perry-stdlib/Cargo.toml as `bundled-uuid + bundled-nanoid`
        // for backwards compat, but feature-set computation goes
        // straight to the per-binding feature.
        "uuid" => &["bundled-uuid"],
        "nanoid" => &["bundled-nanoid"],

        // Slugify gained the `bundled-slugify` feature in v0.5.536 so
        // the well-known flip can swap it out for perry-ext-slugify.
        // Default-on via `default = ["full"]` keeps existing
        // `import 'slugify'` calls byte-identical.
        "slugify" => &["bundled-slugify"],
        // lru-cache: feature-gated v0.5.539; well-known flip
        // routes to perry-ext-lru-cache.
        "lru-cache" => &["bundled-lru-cache"],
        // exponential-backoff: feature-gated v0.5.542 alongside
        // the perry-ffi closure-invocation surface that powers
        // its `backOff(fn)` retry loop.
        "exponential-backoff" => &["bundled-exponential-backoff"],
        // dotenv was always-on through v0.5.532; gated behind
        // `bundled-dotenv` from v0.5.533 onwards so the well-known
        // bindings flip (#466 Phase 4 step 2) can swap perry-stdlib's
        // copy out for `perry-ext-dotenv` without duplicate
        // `_js_dotenv_*` symbols at link time. The well-known path
        // strips this feature from the set; the default path leaves
        // it on so byte-identical behavior is preserved.
        "dotenv" | "dotenv/config" => &["bundled-dotenv"],

        // readline (#347) — needs the async-runtime feature so the
        // event-loop pump tick drains its line / data / keypress
        // queues. Without async-runtime, `import readline` still
        // compiles (rl.close() fires synchronously) but live stdin
        // events won't propagate to user callbacks.
        "readline" => &["async-runtime"],

        // Modules with no optional perry-stdlib dependency (decimal.js,
        // bignumber.js, lru-cache, commander, exponential-backoff, http,
        // https, events, async_hooks, worker_threads, …) — handled by
        // always-on stdlib code.
        _ => &[],
    }
}

/// Compute the union of perry-stdlib features required to cover every
/// native module the project imports, plus features needed to satisfy
/// non-import-based usage flags (e.g. `uses_fetch` ⇒ `http-client`).
pub fn compute_required_features(
    native_module_imports: &BTreeSet<String>,
    uses_fetch: bool,
    uses_crypto_builtins: bool,
) -> BTreeSet<&'static str> {
    let mut features = BTreeSet::new();
    for module in native_module_imports {
        for feat in module_to_features(module) {
            features.insert(*feat);
        }
    }
    // Built-in `fetch()` and `node-fetch` both bottom out in reqwest.
    if uses_fetch {
        features.insert("http-client");
    }
    // Perry's bare `crypto.randomBytes` / `sha256` / etc. builtins bottom
    // out in the perry-stdlib `crypto` feature.
    if uses_crypto_builtins {
        features.insert("crypto");
    }
    features
}

/// Render a feature set as the comma-separated string Cargo expects on
/// `--features`.
pub fn features_to_cargo_arg(features: &BTreeSet<&'static str>) -> String {
    features.iter().copied().collect::<Vec<_>>().join(",")
}

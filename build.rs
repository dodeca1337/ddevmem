//! Build script: minify `src/web_ui.html` into `$OUT_DIR/web_ui.min.html`.
//!
//! The web UI is embedded into the binary via `include_str!`. Authoring it
//! readably (long Carbon-spec comments, descriptive class / function names,
//! generous whitespace) makes the source hover around 35–40 KB. Shipping the
//! verbatim source bloats every release binary that pulls in the `web`
//! feature, even though browsers do not benefit from any of that prose.
//!
//! We minify at build time so:
//!   * `src/web_ui.html` stays the source of truth — keep editing it the way
//!     it is now, with comments, formatting, and full identifiers;
//!   * the binary only carries a stripped, single-line copy;
//!   * Cargo reruns this script whenever the source HTML changes.

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=src/web_ui.html");
    println!("cargo:rerun-if-changed=build.rs");

    let src = fs::read("src/web_ui.html").expect("read src/web_ui.html");

    // Conservative minification: collapse whitespace + drop comments +
    // minify embedded CSS/JS. We do NOT mangle identifiers (the page exposes
    // global helpers like `toggleTheme`, `dumpMap`, etc. wired from inline
    // `onclick="..."` attributes — renaming them would silently break the
    // UI). minify-html's defaults already preserve these.
    //
    // The `allow_*` flags below are technically "possibly non-compliant"
    // (see `Cfg::enable_possibly_noncompliant`) but every modern browser
    // parses the resulting markup correctly, and they bring meaningful size
    // savings (unquoted attributes, optimal entities, dropped attribute
    // spacing, compact `<!doctype html>`).
    let mut cfg = minify_html::Cfg::new();
    cfg.minify_css = true;
    cfg.minify_js = true;
    cfg.keep_comments = false;
    cfg.enable_possibly_noncompliant();

    let min = minify_html::minify(&src, &cfg);

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("web_ui.min.html"), &min).expect("write minified html");
}

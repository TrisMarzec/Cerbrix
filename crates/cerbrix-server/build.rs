//! Embeds the built web UI (`ui/dist`, relative to the workspace root) into the binary.
//!
//! The generated `$OUT_DIR/webui_assets.rs` holds one `Asset` per file, in sorted order, with
//! its bytes included by absolute path. Without `ui/dist/index.html` (no Node was available)
//! a single placeholder page is embedded instead and `PLACEHOLDER` is true, so `cargo build`
//! never needs Node.
//!
//! Environment: `CERBRIX_UI_DIST` points at another UI directory (default `ui/dist` of the
//! workspace; a relative path is taken relative to the workspace root); with
//! `CERBRIX_REQUIRE_UI=1` (or `true`) a missing `index.html` fails the build instead of embedding
//! the placeholder (releases). Any other non-empty value of `CERBRIX_REQUIRE_UI` fails the
//! build, so a typo is not silently ignored. A release build that embeds the placeholder
//! prints a cargo warning.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const PLACEHOLDER_HTML: &str = "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Cerbrix</title>\n</head>\n<body>\n<p>The web UI was not built. Run <code>npm ci &amp;&amp; npm run build</code> in <code>ui/</code> and build the server again.</p>\n</body>\n</html>\n";

fn main() {
    println!("cargo:rerun-if-env-changed=CERBRIX_UI_DIST");
    println!("cargo:rerun-if-env-changed=CERBRIX_REQUIRE_UI");
    println!("cargo:rerun-if-changed=build.rs");

    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let custom = std::env::var_os("CERBRIX_UI_DIST").filter(|v| !v.is_empty());
    let workspace = manifest
        .parent()
        .and_then(Path::parent)
        .expect("the crate sits two levels below the workspace root")
        .to_path_buf();
    // A relative `CERBRIX_UI_DIST` is taken relative to the workspace root; an absolute one
    // replaces the base in `join`.
    let dist = match &custom {
        Some(custom) => workspace.join(custom),
        None => workspace.join("ui/dist"),
    };
    let require_ui = require_ui(std::env::var("CERBRIX_REQUIRE_UI").ok().as_deref())
        .unwrap_or_else(|message| panic!("{message}"));

    if let Some(custom) = &custom {
        println!(
            "cargo:warning=CERBRIX_UI_DIST is set: the web UI is taken from {}",
            custom.to_string_lossy()
        );
    }
    // `ui/dist` always exists in a checkout (it holds a tracked, empty `.gitkeep`), so it is
    // the only watched path: cargo scans its tree, whereas a path that does not exist is
    // always stale. It is never created here.
    println!("cargo:rerun-if-changed={}", dist.display());
    if !dist.is_dir() {
        let hint = if custom.is_none() {
            "; restore it with `git checkout ui/dist/.gitkeep`"
        } else {
            ""
        };
        println!(
            "cargo:warning={} is missing, so cargo rebuilds the server on every call{hint}",
            dist.display()
        );
    }

    let built = dist.join("index.html").is_file();
    if !built && require_ui {
        panic!(
            "CERBRIX_REQUIRE_UI is set but {} does not exist: build the web UI first (`npm ci && npm run build` in ui/)",
            dist.join("index.html").display()
        );
    }
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    if built {
        collect(&dist, "", &mut files);
        files.sort();
        // The directory scan sees new and changed files but not a removed one; a watched file
        // that is gone is stale, so every embedded file is watched too.
        for (_, file) in &files {
            println!("cargo:rerun-if-changed={}", file.display());
        }
    } else {
        let placeholder = out_dir.join("placeholder.html");
        write_if_changed(&placeholder, PLACEHOLDER_HTML.as_bytes());
        files.push(("index.html".to_owned(), placeholder));
        if std::env::var("PROFILE").is_ok_and(|p| p == "release") {
            println!(
                "cargo:warning=this release build embeds a placeholder page instead of the web UI ({} has no index.html); build the UI first (`npm ci && npm run build` in ui/) and build the server again, or set CERBRIX_REQUIRE_UI=1 to make this an error",
                dist.display()
            );
        }
    }

    let mut code = String::new();
    writeln!(code, "pub const PLACEHOLDER: bool = {};", !built).unwrap();
    writeln!(code, "pub static ASSETS: &[Asset] = &[").unwrap();
    for (path, file) in &files {
        let bytes =
            fs::read(file).unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
        let digest = Sha256::digest(&bytes);
        let etag: String = digest.iter().take(8).fold(String::new(), |mut s, b| {
            write!(s, "{b:02x}").unwrap();
            s
        });
        // `{:?}` of a string is a valid Rust string literal with backslashes escaped.
        let absolute = file.canonicalize().unwrap_or_else(|_| file.clone());
        writeln!(
            code,
            "    Asset {{ path: {path:?}, bytes: include_bytes!({:?}), content_type: {:?}, etag: {:?} }},",
            plain_path(&absolute),
            content_type(path),
            format!("\"{etag}\""),
        )
        .unwrap();
    }
    writeln!(code, "];").unwrap();
    write_if_changed(&out_dir.join("webui_assets.rs"), code.as_bytes());
}

/// `CERBRIX_REQUIRE_UI`: unset or empty is off, `1` and `true` are on, anything else is an error.
fn require_ui(value: Option<&str>) -> Result<bool, String> {
    match value {
        None | Some("") => Ok(false),
        Some("1" | "true") => Ok(true),
        Some(other) => Err(format!(
            "CERBRIX_REQUIRE_UI={other:?} is not understood: use 1 or true to require the web UI, or leave it unset"
        )),
    }
}

/// Writes `content` only when the file differs, so an unchanged result keeps its mtime.
fn write_if_changed(path: &Path, content: &[u8]) {
    if fs::read(path).is_ok_and(|old| old == content) {
        return;
    }
    fs::write(path, content).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

/// The path as text for `include_bytes!`. The Windows verbatim prefix of a drive path
/// (`\\?\C:\...`) is dropped; a UNC path (`\\?\UNC\...`) keeps it.
fn plain_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.to_owned(),
        _ => text.into_owned(),
    }
}

/// Collects every file below `dir` as (path relative to `ui/dist` with `/`, absolute path).
/// Hidden entries, source maps and symbolic links are skipped.
fn collect(dir: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("cannot read a directory entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.ends_with(".map") {
            continue;
        }
        // `file_type` does not follow symbolic links: a link is neither embedded nor entered.
        let kind = entry.file_type().expect("cannot read a file type");
        if kind.is_dir() {
            collect(&entry.path(), &format!("{prefix}{name}/"), files);
        } else if kind.is_file() {
            files.push((format!("{prefix}{name}"), entry.path()));
        }
    }
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("txt") => "text/plain; charset=utf-8",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

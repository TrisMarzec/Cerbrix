//! Embeds the built web UI (`ui/dist`, relative to the workspace root) into the binary.
//!
//! The generated `$OUT_DIR/webui_assets.rs` holds one `Asset` per file, in sorted order, with
//! its bytes included by absolute path. Without `ui/dist/index.html` (no Node was available)
//! a single placeholder page is embedded instead and `PLACEHOLDER` is true, so `cargo build`
//! never needs Node.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

const PLACEHOLDER_HTML: &str = "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Cerbrix</title>\n</head>\n<body>\n<p>The web UI was not built. Run <code>npm ci &amp;&amp; npm run build</code> in <code>ui/</code> and build the server again.</p>\n</body>\n</html>\n";

fn main() {
    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let dist = manifest.join("../../ui/dist");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", dist.display());

    let built = dist.join("index.html").is_file();
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    if built {
        collect(&dist, "", &mut files);
        files.sort();
    } else {
        let placeholder = out_dir.join("placeholder.html");
        fs::write(&placeholder, PLACEHOLDER_HTML).expect("cannot write the placeholder page");
        files.push(("index.html".to_owned(), placeholder));
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
            absolute.to_string_lossy().trim_start_matches(r"\?\"),
            content_type(path),
            format!("\"{etag}\""),
        )
        .unwrap();
        println!("cargo:rerun-if-changed={}", file.display());
    }
    writeln!(code, "];").unwrap();
    fs::write(out_dir.join("webui_assets.rs"), code).expect("cannot write webui_assets.rs");
}

/// Collects every file below `dir` as (path relative to `ui/dist` with `/`, absolute path).
fn collect(dir: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("cannot read a directory entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if path.is_dir() {
            collect(&path, &format!("{prefix}{name}/"), files);
        } else {
            files.push((format!("{prefix}{name}"), path));
        }
    }
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("woff2") => "font/woff2",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("txt") => "text/plain; charset=utf-8",
        Some("json") => "application/json",
        Some("map") => "application/json",
        _ => "application/octet-stream",
    }
}

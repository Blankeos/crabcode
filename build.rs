use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=remote-client/dist/client");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let dist_dir = manifest_dir.join("remote-client/dist/client");
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("remote_assets.rs");

    if !dist_dir.join("index.html").is_file() {
        panic!(
            "remote client assets are missing; run `just remote-client-build` before building crabcode"
        );
    }

    let mut files = Vec::new();
    collect_files(&dist_dir, &dist_dir, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut output = String::new();
    output.push_str(
        "pub struct RemoteAsset {\n    pub content_type: &'static str,\n    pub body: &'static [u8],\n}\n\n",
    );
    output.push_str("pub fn remote_asset(path: &str) -> Option<RemoteAsset> {\n");
    output.push_str("    match path {\n");

    if files.is_empty() {
        output.push_str("        _ => None,\n");
    } else {
        for (route, file_path) in files {
            let route = route.replace('\\', "/");
            let content_type = content_type_for_path(&route);
            let path_literal = file_path.display().to_string().replace('\\', "\\\\");
            output.push_str(&format!(
                "        {:?} => Some(RemoteAsset {{ content_type: {:?}, body: include_bytes!({:?}) }}),\n",
                route, content_type, path_literal
            ));

            if route == "/index.html" {
                output.push_str(&format!(
                    "        \"/\" => Some(RemoteAsset {{ content_type: {:?}, body: include_bytes!({:?}) }}),\n",
                    content_type, path_literal
                ));
            }
        }
        output.push_str("        _ => None,\n");
    }

    output.push_str("    }\n}\n");

    fs::write(out_path, output).unwrap();
}

fn collect_files(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }

        if path.is_dir() {
            collect_files(root, &path, files);
        } else if path.is_file() {
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let route = format!("/{}", relative.display());
            files.push((route, path));
        }
    }
}

fn content_type_for_path(path: &str) -> &'static str {
    match Path::new(path).extension().and_then(|ext| ext.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

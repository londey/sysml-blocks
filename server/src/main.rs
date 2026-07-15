//! sysml-blocks server
//!
//! Serves the block-editor web UI and a small JSON API over a workspace of
//! .sysml files (typically a Docker-mapped volume at /models).
//!
//!   GET  /api/model            full parsed workspace as JSON
//!   GET  /api/source?file=...  raw text of one file
//!   GET  /api/export?scope=... PDF export (scope=element|file|project,
//!                              id=/file= selector, format=doc|blocks)
//!   POST /api/edit             apply one EditOp (see model.rs), returns model
//!   anything else              static files from WEB_ROOT (SPA)

mod export;
mod lexer;
mod model;
mod parser;
mod pdf;

use export::{ExportFormat, ExportScope};
use model::{EditOp, Workspace};
#[allow(unused_imports)]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tiny_http::{Header, Method, Response, Server};

fn header(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap()
}

fn json_response(status: u32, body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_status_code(status)
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
        .with_header(header("Cache-Control", "no-store"))
}

fn main() {
    let models_dir = std::env::var("MODELS_DIR").unwrap_or_else(|_| "/models".into());
    let web_root = std::env::var("WEB_ROOT").unwrap_or_else(|_| "/app/web".into());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let models_dir = PathBuf::from(models_dir);
    if !models_dir.exists() {
        eprintln!(
            "models directory {:?} does not exist — mount a volume there",
            models_dir
        );
        std::process::exit(1);
    }

    let ws = Mutex::new(Workspace::load(&models_dir));
    {
        let w = ws.lock().unwrap();
        println!(
            "indexed {} .sysml file(s) under {:?}",
            w.files.len(),
            models_dir
        );
    }

    let server = Server::http(("0.0.0.0", port)).expect("failed to bind");
    println!("sysml-blocks listening on http://localhost:{}", port);

    for mut req in server.incoming_requests() {
        let url = req.url().to_string();
        let path_only = url.split('?').next().unwrap_or("").to_string();
        let method = req.method().clone();

        let resp = match (method, path_only.as_str()) {
            (Method::Get, "/api/model") => {
                let mut w = ws.lock().unwrap();
                w.refresh(&models_dir); // pick up external changes (git, sync)
                json_response(200, serde_json::to_string(&*w).unwrap())
            }
            (Method::Get, "/api/source") => {
                let file = query_param(&url, "file");
                let w = ws.lock().unwrap();
                match file.and_then(|f| {
                    w.files.iter().find(|fm| fm.path == f).map(|fm| fm.source.clone())
                }) {
                    Some(src) => Response::from_string(src)
                        .with_header(header("Content-Type", "text/plain; charset=utf-8"))
                        .with_header(header("Cache-Control", "no-store")),
                    None => Response::from_string("file not found").with_status_code(404),
                }
            }
            (Method::Get, "/api/export") => {
                let mut w = ws.lock().unwrap();
                w.refresh(&models_dir);
                handle_export(&w, &url)
            }
            (Method::Post, "/api/edit") => {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                match serde_json::from_str::<EditOp>(&body) {
                    Ok(op) => {
                        let mut w = ws.lock().unwrap();
                        w.refresh(&models_dir);
                        match w.apply(&models_dir, &op) {
                            Ok(()) => json_response(200, serde_json::to_string(&*w).unwrap()),
                            Err(e) => json_response(
                                409,
                                format!("{{\"error\":{}}}", serde_json::to_string(&e).unwrap()),
                            ),
                        }
                    }
                    Err(e) => json_response(
                        400,
                        format!(
                            "{{\"error\":{}}}",
                            serde_json::to_string(&e.to_string()).unwrap()
                        ),
                    ),
                }
            }
            (Method::Get, p) => serve_static(Path::new(&web_root), p),
            _ => Response::from_string("method not allowed").with_status_code(405),
        };
        let _ = req.respond(resp);
    }
}

/// GET /api/export?scope=element|file|project[&id=..][&file=..][&format=doc|blocks]
/// Success: the PDF as an attachment. Bad params: 400. Unknown id/file: 404.
fn handle_export(w: &Workspace, url: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    fn err400(msg: &str) -> Response<std::io::Cursor<Vec<u8>>> {
        json_response(
            400,
            format!("{{\"error\":{}}}", serde_json::to_string(msg).unwrap()),
        )
    }
    let format = match query_param(url, "format").as_deref() {
        None | Some("doc") => ExportFormat::Doc,
        Some("blocks") => ExportFormat::Blocks,
        Some(_) => return err400("format must be 'doc' or 'blocks'"),
    };
    let scope = match query_param(url, "scope").as_deref() {
        Some("element") => match query_param(url, "id") {
            Some(id) => ExportScope::Element(id),
            None => return err400("scope=element requires an id parameter"),
        },
        Some("file") => match query_param(url, "file") {
            Some(f) => ExportScope::File(f),
            None => return err400("scope=file requires a file parameter"),
        },
        Some("project") => ExportScope::Project,
        Some(_) => return err400("scope must be 'element', 'file' or 'project'"),
        None => return err400("missing scope parameter"),
    };
    match export::export_pdf(w, &scope, format) {
        Ok((bytes, name)) => Response::from_data(bytes)
            .with_header(header("Content-Type", "application/pdf"))
            .with_header(header(
                "Content-Disposition",
                &format!("attachment; filename=\"{}\"", name),
            ))
            .with_header(header("Cache-Control", "no-store")),
        Err(e) => json_response(
            404,
            format!("{{\"error\":{}}}", serde_json::to_string(&e).unwrap()),
        ),
    }
}

/// Extract and url-decode one query parameter from a request url.
fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split('?').nth(1)?;
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        if k == key {
            Some(urldecode(v))
        } else {
            None
        }
    })
}

fn serve_static(root: &Path, path: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let rel = path.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    let candidate = root.join(rel);
    // path traversal guard
    let ok = candidate
        .canonicalize()
        .map(|c| c.starts_with(root.canonicalize().unwrap_or_else(|_| root.into())))
        .unwrap_or(false);
    let candidate = if ok && candidate.is_file() {
        candidate
    } else {
        root.join("index.html")
    };
    match std::fs::read(&candidate) {
        Ok(bytes) => {
            let mime = match candidate.extension().and_then(|e| e.to_str()) {
                Some("html") => "text/html; charset=utf-8",
                Some("js") => "application/javascript",
                Some("css") => "text/css",
                Some("svg") => "image/svg+xml",
                Some("png") => "image/png",
                Some("json") => "application/json",
                _ => "application/octet-stream",
            };
            Response::from_data(bytes).with_header(header("Content-Type", mime))
        }
        Err(_) => Response::from_string("web assets not found").with_status_code(404),
    }
}

fn urldecode(s: &str) -> String {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        if b[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(b[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

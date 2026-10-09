//! Model downloads through the vendored hf-hub (vendor/hf-hub, issue #507).
//!
//! A local server stands in for the Hugging Face Hub and its Xet CDN, with
//! the CDN behaviour from the report: a ranged request gets a plain `200`
//! with the whole file and no `Content-Range`. The Hub's redirect carries
//! `X-Linked-Size`, as the real one does.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;

use hf_hub::api::sync::ApiBuilder;
use hf_hub::{Cache, Repo, RepoType};

const BODY: &[u8] = b"pretend this is model.onnx";
const ETAG: &str = "0123456789abcdef";

/// How the stand-in CDN treats `Range`.
#[derive(Clone, Copy)]
enum Cdn {
    /// Never honours it, not even the `bytes=0-0` size probe (the report).
    IgnoresRange,
    /// Answers the size probe properly, ignores `Range` on the download.
    IgnoresRangeOnResume,
}

fn serve(listener: TcpListener, cdn: Cdn) {
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            answer(stream, port, cdn);
        }
    });
}

fn answer(mut stream: TcpStream, port: u16, cdn: Cdn) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut range = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("range")
        {
            range = value.trim().to_string();
        }
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    let response = if path.contains("/resolve/") {
        // The Hub: redirect to the CDN, announcing the size and the etag.
        format!(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/cdn/blob\r\n\
             X-Repo-Commit: 1111111111111111111111111111111111111111\r\n\
             X-Linked-Etag: \"{ETAG}\"\r\nX-Linked-Size: {}\r\n\
             Content-Length: 0\r\nConnection: close\r\n\r\n",
            BODY.len()
        )
        .into_bytes()
    } else if matches!(cdn, Cdn::IgnoresRangeOnResume) && range == "bytes=0-0" {
        let mut r = format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-0/{}\r\n\
             Content-Length: 1\r\nConnection: close\r\n\r\n",
            BODY.len()
        )
        .into_bytes();
        r.push(BODY[0]);
        r
    } else {
        // The CDN: ignores Range, no Content-Range.
        let mut r = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            BODY.len()
        )
        .into_bytes();
        r.extend_from_slice(BODY);
        r
    };
    let _ = stream.write_all(&response);
}

fn api(cache: &Path, cdn: Cdn) -> hf_hub::api::sync::Api {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    serve(listener, cdn);
    ApiBuilder::new()
        .with_token(None)
        .with_endpoint(endpoint)
        .with_cache_dir(cache.to_path_buf())
        .with_progress(false)
        .with_retries(0)
        .build()
        .unwrap()
}

#[test]
fn a_cdn_answering_200_without_content_range_still_downloads() {
    let cache = tempfile::tempdir().unwrap();
    let path = api(cache.path(), Cdn::IgnoresRange)
        .model("org/model".to_string())
        .get("model.onnx")
        .expect("unpatched hf-hub 0.5.0 fails here with MissingHeader(Content-Range)");
    assert_eq!(std::fs::read(path).unwrap(), BODY);
}

#[test]
fn a_resumed_download_from_a_server_ignoring_range_is_not_corrupted() {
    let cache = tempfile::tempdir().unwrap();
    // A partial download left by an earlier interrupted run.
    let blob = Cache::new(cache.path().to_path_buf())
        .repo(Repo::new("org/model".to_string(), RepoType::Model))
        .blob_path(ETAG);
    std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
    std::fs::write(blob.with_extension("part"), b"pretend").unwrap();

    let path = api(cache.path(), Cdn::IgnoresRangeOnResume)
        .model("org/model".to_string())
        .get("model.onnx")
        .unwrap();
    // Not "pretend" followed by the whole file again.
    assert_eq!(std::fs::read(path).unwrap(), BODY);
}

/// The real Hub and its Xet CDN, for the default model's `model.onnx`
/// (545,851 bytes; the weights are in `model.onnx_data`). Needs the network:
/// `cargo test -p icm-core --test hf_hub_xet -- --ignored`.
#[test]
#[ignore = "needs network access to huggingface.co"]
fn the_default_model_file_downloads_from_the_real_hub() {
    let cache = tempfile::tempdir().unwrap();
    // Anonymous: never the token of the machine running the test.
    let api = ApiBuilder::new()
        .with_token(None)
        .with_cache_dir(cache.path().to_path_buf())
        .with_progress(false)
        .build()
        .unwrap();
    let path = api
        .model("Qdrant/multilingual-e5-large-onnx".to_string())
        .get("model.onnx")
        .unwrap();
    assert_eq!(std::fs::metadata(path).unwrap().len(), 545_851);
}

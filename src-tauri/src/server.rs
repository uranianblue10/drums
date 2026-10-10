//! LAN server so the phone can play too: serves the bundled frontend on 0.0.0.0:PORT.
use axum::{
    extract::State,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use include_dir::{include_dir, Dir};
use tokio::{net::TcpListener, sync::oneshot};

use crate::favs::FavStore;

pub(crate) static FRONTEND: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../frontend");

pub const PORT: u16 = 8909;

/// Decode an embedded opus asset to a 16-bit PCM WAV (stereo 44.1kHz) via
/// ffmpeg, with an on-disk cache. iOS Safari cannot decode Ogg Opus in either
/// Web Audio or `<audio>`, so the mobile frontend plays these WAVs instead.
pub fn opus_to_wav(opus: &[u8], tag: &str) -> Result<Vec<u8>, String> {
    let dir = std::env::temp_dir().join("loopz-cache");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cache dir: {e}"))?;
    let safe: String = tag
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let in_path = dir.join(format!("{safe}.opus"));
    let wav_path = dir.join(format!("{safe}.wav"));
    // Reuse the transcoded output if the source didn't change.
    let existing = std::fs::metadata(&in_path).ok().map(|m| m.len());
    if existing != Some(opus.len() as u64) {
        std::fs::write(&in_path, opus).map_err(|e| format!("write tmp: {e}"))?;
        let _ = std::fs::remove_file(&wav_path);
    }
    if std::fs::metadata(&wav_path).is_err() {
        let status = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-i",
                &in_path.to_string_lossy(),
                "-map",
                "0:a",
                "-ac",
                "2",
                "-ar",
                "44100",
                "-f",
                "wav",
                &wav_path.to_string_lossy(),
            ])
            .status()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    "ffmpeg not found - install it (pacman -S ffmpeg)".to_string()
                } else {
                    format!("ffmpeg failed to start: {e}")
                }
            })?;
        if !status.success() {
            return Err("ffmpeg could not transcode this loop".to_string());
        }
    }
    let wav = std::fs::read(&wav_path).map_err(|e| format!("read wav: {e}"))?;
    if wav.len() < 44 {
        return Err("transcoded audio is empty".to_string());
    }
    Ok(wav)
}

async fn serve_wav(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');
    let src = path.strip_prefix("wav/").unwrap_or("");
    if src.is_empty() || src.split('/').any(|s| s == "..") || !src.ends_with(".opus") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let Some(f) = FRONTEND.get_file(src) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    // ffmpeg blocks; keep it off the async runtime.
    let bytes = f.contents().to_vec();
    let tag = src.to_string();
    let wav = tokio::task::spawn_blocking(move || opus_to_wav(&bytes, &tag))
        .await
        .map_err(|e| format!("transcode task: {e}"))
        .and_then(|r| r);
    match wav {
        Ok(wav) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "audio/wav")],
            wav,
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

pub struct RunningServer {
    pub url: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
    handle: tokio::task::JoinHandle<()>,
}

async fn serve_file(uri: Uri) -> impl IntoResponse {
    let mut path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        path = "index.html";
    }
    if path.split('/').any(|s| s == "..") {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    match FRONTEND.get_file(path) {
        Some(f) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime.as_ref())],
                f.contents(),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn fav_list_api(State(favs): State<FavStore>) -> impl IntoResponse {
    Json(favs.list())
}

#[derive(serde::Deserialize)]
struct FavSetReq {
    file: String,
    fav: bool,
}

async fn fav_set_api(
    State(favs): State<FavStore>,
    Json(req): Json<FavSetReq>,
) -> impl IntoResponse {
    if req.file.is_empty() || req.file.split('/').any(|s| s == "..") {
        return (StatusCode::BAD_REQUEST, "bad file").into_response();
    }
    Json(favs.set(req.file, req.fav)).into_response()
}

pub fn router(favs: FavStore) -> Router {
    Router::new()
        .route("/api/favorites", get(fav_list_api).post(fav_set_api))
        .route("/wav/*path", get(serve_wav))
        .fallback(get(serve_file))
        .with_state(favs)
}

pub async fn serve_on(
    listener: TcpListener,
    shutdown: oneshot::Receiver<()>,
    favs: FavStore,
) {
    axum::serve(listener, router(favs))
        .with_graceful_shutdown(async {
            shutdown.await.ok();
        })
        .await
        .ok();
}

/// Bind 0.0.0.0:PORT and serve in the background. Returns the LAN URL.
pub async fn start(favs: FavStore) -> Result<RunningServer, String> {
    let listener = TcpListener::bind(("0.0.0.0", PORT))
        .await
        .map_err(|e| format!("Could not bind port {PORT} (is loopz.py still running?): {e}"))?;
    let (tx, rx) = oneshot::channel();
    let handle = tokio::spawn(serve_on(listener, rx, favs));
    let ip =
        local_ip_address::local_ip().map_err(|e| format!("Could not determine LAN IP: {e}"))?;
    Ok(RunningServer {
        url: format!("http://{ip}:{PORT}"),
        shutdown_tx: Some(tx),
        handle,
    })
}

impl RunningServer {
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), &mut self.handle).await;
        self.handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn get(addr: std::net::SocketAddr, path: &str) -> (String, Vec<u8>) {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(format!("GET {path} HTTP/1.0\r\nHost: x\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| i + 4)
            .unwrap_or(0);
        (
            String::from_utf8_lossy(&buf[..end]).into_owned(),
            buf[end..].to_vec(),
        )
    }

    async fn post_json(
        addr: std::net::SocketAddr,
        path: &str,
        body: &str,
    ) -> (String, Vec<u8>) {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let req = format!(
            "POST {path} HTTP/1.0\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| i + 4)
            .unwrap_or(0);
        (
            String::from_utf8_lossy(&buf[..end]).into_owned(),
            buf[end..].to_vec(),
        )
    }

    fn test_store() -> FavStore {
        let dir = std::env::temp_dir().join(format!("loopz-servtest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        FavStore::load(dir)
    }

    #[tokio::test]
    async fn serves_bundled_frontend() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (_keep, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(serve_on(listener, rx, test_store()));

        let (head, body) = get(addr, "/loops.json").await;
        assert!(head.contains(" 200 OK"), "loops.json: {head}");
        assert!(head.contains("audio/opus") || head.contains("application/json") || head.contains("octet-stream"), "mime: {head}");
        assert!(body.len() > 1000, "loops.json body too small");
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 120);

        let (head, _) = get(addr, "/").await;
        assert!(head.contains(" 200 OK"), "index: {head}");

        let (head, body) = get(addr, "/apple-touch-icon.png").await;
        assert!(head.contains(" 200 OK"), "touch icon: {head}");
        assert!(head.contains("image/png"), "touch icon mime: {head}");
        assert_eq!(
            &body[..8],
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
            "not a PNG file"
        );

        let (head, body) = get(addr, "/manifest.json").await;
        assert!(head.contains(" 200 OK"), "manifest: {head}");
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["short_name"], "Loopz");

        let (head, body) = get(addr, "/loops/amen.opus").await;
        assert!(head.contains(" 200 OK"), "opus: {head}");
        assert!(!body.is_empty());

        let (head, body) = get(addr, "/wav/loops/amen.opus").await;
        assert!(head.contains(" 200 OK"), "wav: {head}");
        assert!(
            head.contains("audio/wav"),
            "wav mime: {head}"
        );
        assert!(body.len() > 100, "wav body too small");
        assert_eq!(&body[0..4], b"RIFF", "not a WAV file");
        assert_eq!(&body[8..12], b"WAVE", "not a WAV file");

        let (head, _) = get(addr, "/wav/loops/does-not-exist.opus").await;
        assert!(head.contains(" 404"), "wav 404: {head}");

        let (head, _) = get(addr, "/does-not-exist").await;
        assert!(head.contains(" 404"), "404: {head}");

        task.abort();
    }

    #[tokio::test]
    async fn favorites_api_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (_keep, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(serve_on(listener, rx, test_store()));

        let (head, body) = get(addr, "/api/favorites").await;
        assert!(head.contains(" 200 OK"), "fav list: {head}");
        assert_eq!(serde_json::from_slice::<Vec<String>>(&body).unwrap(), Vec::<String>::new());

        let (head, body) = post_json(
            addr,
            "/api/favorites",
            r#"{"file":"loops/amen.opus","fav":true}"#,
        )
        .await;
        assert!(head.contains(" 200 OK"), "fav set: {head}");
        assert_eq!(
            serde_json::from_slice::<Vec<String>>(&body).unwrap(),
            vec!["loops/amen.opus".to_string()]
        );

        // Second client sees it too (shared, not per-connection).
        let (_, body) = get(addr, "/api/favorites").await;
        assert_eq!(
            serde_json::from_slice::<Vec<String>>(&body).unwrap(),
            vec!["loops/amen.opus".to_string()]
        );

        let (head, body) = post_json(
            addr,
            "/api/favorites",
            r#"{"file":"loops/amen.opus","fav":false}"#,
        )
        .await;
        assert!(head.contains(" 200 OK"), "fav unset: {head}");
        assert!(serde_json::from_slice::<Vec<String>>(&body).unwrap().is_empty());

        task.abort();
    }
}

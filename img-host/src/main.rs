use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Multipart, Path as AxPath, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use tokio::fs;use tokio::net::TcpListener;
use uuid::Uuid;

struct Config {
    storage_dir: PathBuf,
    base_url: String,
    max_age: Duration,
    max_file_size: u64,
    max_total_size: u64,
}

impl Config {
    fn from_env() -> Self {
        let storage_dir = env_or("STORAGE_DIR", "./img-cache");
        let base_url = env_or("BASE_URL", "").trim_end_matches('/').to_string();
        let max_age = parse_dur_or(env_or("MAX_AGE", "1h"), Duration::from_secs(3600));
        let max_file_size = parse_bytes(env_or("MAX_FILE_SIZE", "500MB"));
        let max_total_size = parse_bytes(env_or("MAX_TOTAL_SIZE", "0"));
        Config {
            storage_dir: PathBuf::from(storage_dir),
            base_url,
            max_age,
            max_file_size,
            max_total_size,
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn parse_dur_or(s: String, default: Duration) -> Duration {
    // Allow a bare integer to mean seconds, else fall back to parse_duration.
    match s.trim().parse::<u64>() {
        Ok(secs) if !s.contains(':') && !s.is_empty() => Duration::from_secs(secs),
        _ => parse_duration::parse(&s).unwrap_or(default),
    }
}

fn parse_bytes(s: String) -> u64 {
    let t = s.trim().to_ascii_lowercase();
    let (num, mult) = if let Some(v) = t.strip_suffix("tb") {
        (v, 1_u64 << 40)
    } else if let Some(v) = t.strip_suffix("gb") {
        (v, 1_u64 << 30)
    } else if let Some(v) = t.strip_suffix("mb") {
        (v, 1_u64 << 20)
    } else if let Some(v) = t.strip_suffix("kb") {
        (v, 1_u64 << 10)
    } else {
        (t.as_str(), 1)
    };
    num.trim().parse::<u64>().unwrap_or(0).saturating_mul(mult)
}

#[derive(Clone)]
struct AppState {
    cfg: std::sync::Arc<Config>,
}

#[derive(Serialize)]
struct Health {
    ok: bool,
}

#[derive(Serialize)]
struct UploadResp {
    url: String,
}

#[tokio::main]
async fn main() {
    let cfg = std::sync::Arc::new(Config::from_env());

    fs::create_dir_all(&cfg.storage_dir).await.expect("create storage dir");

    // Kick off periodic sweeps (expiry + total-size eviction).
    {
        let cfg = cfg.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(60));
            loop {
                ticker.tick().await;
                sweep(&cfg).await;
            }
        });
    }

    let state = AppState { cfg: cfg.clone() };

    let app = Router::new()
        .route("/", get(health))
        .route("/api/upload", post(upload))
        .route("/i/{name}", get(serve).delete(remove))
        .with_state(state)
        .layer(DefaultBodyLimit::max(cfg.max_file_size as usize));

    let addr: SocketAddr = match env_or("LISTEN_ADDR", "0.0.0.0:8080").parse() {
        Ok(a) => a,
        Err(_) => {
            eprintln!("invalid LISTEN_ADDR, using 0.0.0.0:8080");
            "0.0.0.0:8080".parse().unwrap()
        }
    };

    let listener = TcpListener::bind(addr).await.expect("bind");
    eprintln!("img-host listening on {addr}, dir={}", cfg.storage_dir.display());
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await.expect("server");
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn health() -> Json<Health> {
    Json(Health { ok: true })
}

async fn upload(State(state): State<AppState>, mut multipart: Multipart) -> Result<Json<UploadResp>, Response> {
    let cfg = &state.cfg;
    let field = multipart
        .next_field()
        .await
        .map_err(|_| err(StatusCode::BAD_REQUEST, "invalid multipart"))?
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "no file part"))?;

    let Some(bytes) = field.bytes().await.ok() else {
        return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "upload too large or truncated"));
    };

    if bytes.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "empty file"));
    }
    if bytes.len() as u64 > cfg.max_file_size {
        return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "file exceeds MAX_FILE_SIZE"));
    }

    let ext = extension_for(&bytes);
    let name = format!("{}{}", Uuid::new_v4().simple(), ext);
    let path = cfg.storage_dir.join(&name);

    fs::write(&path, &bytes)
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "write failed"))?;

    // Best-effort total-size enforcement after write.
    if cfg.max_total_size > 0 {
        evict_over_limit(cfg).await;
    }

    let url = format!("{}/i/{}", cfg.base_url, name);
    Ok(Json(UploadResp { url }))
}

fn extension_for(bytes: &[u8]) -> String {
    // Sniff a few common signatures ourselves for nicer extensions.
    if bytes.starts_with(b"\xff\xd8\xff") {
        ".jpg".to_string()
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        ".png".to_string()
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        ".gif".to_string()
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        ".webp".to_string()
    } else if bytes.starts_with(b"BM") {
        ".bmp".to_string()
    } else if bytes.starts_with(b"II*\x00") || bytes.starts_with(b"MM\x00*") {
        ".tiff".to_string()
    } else if bytes.starts_with(b"%PDF") {
        ".pdf".to_string()
    } else if bytes.starts_with(b"<svg") || looks_like_svg(bytes) {
        ".svg".to_string()
    } else if bytes.starts_with(b"\x00\x00\x00 ftypavif") || bytes.get(4..8) == Some(b"ftyp") && bytes.get(8..12) == Some(b"avif") {
        ".avif".to_string()
    } else if bytes.get(4..8) == Some(b"ftyp") {
        ".heic".to_string()
    } else {
        ".bin".to_string()
    }
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let s = bytes.iter().take(256).map(|&b| b as char).collect::<String>().to_ascii_lowercase();
    s.contains("<svg")
}

fn content_type_for(name: &str) -> &'static str {
    let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "heic" => "image/heic",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn err(status: StatusCode, msg: &'static str) -> Response {
    (status, msg).into_response()
}

fn safe_name(part: &str) -> Option<String> {
    // Must be a single file token (no slashes / path traversal) and match our format.
    let p = Path::new(part);
    if p.components().count() != 1 {
        return None;
    }
    let s = p.file_name()?;
    let s = s.to_str()?;
    Some(s.to_string())
}

async fn serve(
    State(state): State<AppState>,
    AxPath(name): AxPath<String>,
) -> Result<Response, Response> {
    let name = safe_name(&name).ok_or_else(|| err(StatusCode::BAD_REQUEST, "invalid name"))?;
    let path = state.cfg.storage_dir.join(&name);

    let meta = fs::metadata(&path)
        .await
        .map_err(|_| err(StatusCode::NOT_FOUND, "not found"))?;

    if !meta.is_file() {
        return Err(err(StatusCode::NOT_FOUND, "not found"));
    }

    // Expire if older than max_age.
    let age = meta
        .modified()
        .map(|m| m.elapsed().unwrap_or(Duration::ZERO))
        .unwrap_or(Duration::ZERO);
    if age > state.cfg.max_age {
        let _ = fs::remove_file(&path).await;
        return Err(err(StatusCode::GONE, "expired"));
    }

    let bytes = fs::read(&path)
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "read failed"))?;

    let delete_at = std::time::SystemTime::now() + state.cfg.max_age;
    let delete_at_http = httpdate(delete_at);

    let ctype = content_type_for(&name);
    let mut res = ([(header::CONTENT_TYPE, ctype)], Bytes::from(bytes)).into_response();

    res.headers_mut()
        .insert("X-Delete-After", HeaderValue::from_str(&delete_at_http).unwrap());
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));

    Ok(res)
}
fn httpdate(t: std::time::SystemTime) -> String {
    // Minimal HTTP-date (RFC 7231) using std only.
    let secs = t.duration_since(std::time::UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs() as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Civil date from Unix days (Howard Hinnant algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d0 = doy - (153 * mp + 2) / 5 + 1;
    let m0 = if mp < 10 { mp + 3 } else { mp - 9 };
    let y0 = y + if m0 <= 2 { 1 } else { 0 };
    let wd = (days + 4).rem_euclid(7); // 0=Sunday
    const WDS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MOS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        WDS[wd as usize], d0, MOS[(m0 as usize) - 1], y0, h, m, s
    )
}

async fn remove(
    State(state): State<AppState>,
    AxPath(name): AxPath<String>,
) -> Result<StatusCode, Response> {
    let name = safe_name(&name).ok_or_else(|| err(StatusCode::BAD_REQUEST, "invalid name"))?;
    let path = state.cfg.storage_dir.join(&name);
    fs::remove_file(&path)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| err(StatusCode::NOT_FOUND, "not found"))
}

async fn sweep(cfg: &Config) {
    let now_delete_before = std::time::SystemTime::now()
        .checked_sub(cfg.max_age)
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

    let mut entries: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    let Ok(mut rd) = fs::read_dir(&cfg.storage_dir).await else {
        return;
    };

    loop {
        let Ok(Some(entry)) = rd.next_entry().await else { break };
        let Ok(ft) = entry.file_type().await else { continue };
        if !ft.is_file() {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = fs::metadata(&path).await else { continue };
        let Ok(modified) = meta.modified() else { continue };

        // Expire by age.
        if modified < now_delete_before {
            let _ = fs::remove_file(&path).await;
            continue;
        }
        entries.push((path, modified));
    }

    // Enforce total-size cap (evict oldest first).
    if cfg.max_total_size > 0 {
        evict_over_limit(cfg).await;
    }
}

async fn evict_over_limit(cfg: &Config) {
    // Read dir, sort by mtime asc, delete newest-survivors-oldest until under cap.
    let Ok(mut rd) = fs::read_dir(&cfg.storage_dir).await else {
        return;
    };
    let mut files: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();
    loop {
        let Ok(Some(entry)) = rd.next_entry().await else { break };
        let Ok(ft) = entry.file_type().await else { continue };
        if !ft.is_file() {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = fs::metadata(&path).await else { continue };
        let Ok(mt) = meta.modified() else { continue };
        files.push((path, mt, meta.len()));
    }
    let total: u64 = files.iter().map(|f| f.2).sum();
    if total <= cfg.max_total_size {
        return;
    }
    files.sort_by(|a, b| a.1.cmp(&b.1)); // oldest first
    let mut remaining = total;
    for (path, _, len) in files {
        if remaining <= cfg.max_total_size {
            break;
        }
        // Drop the oldest file outright until we are back under the cap.
        let _ = fs::remove_file(&path).await;
        remaining = remaining.saturating_sub(len);
    }
}

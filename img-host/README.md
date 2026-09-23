# img-host

Minimal Rust image host: upload an image, get a URL, the file auto-deletes after a configurable TTL (default 1 hour). Ships as a tiny static Docker image (~2 MB), idle RAM in the low single-digit MBs.

## API

All paths are under the server root. A single random identifier is generated per upload; the file's URL is stable for as long as the file lives. File names are URL-safe and collision-free.

| Method | Path         | Description                                  |
|--------|--------------|----------------------------------------------|
| GET    | `/`          | Liveness probe.                              |
| POST   | `/api/upload`| Upload an image, receive its public URL.     |
| GET    | `/i/<token>` | Download a stored file by its token.         |
| DELETE | `/i/<token>` | Delete a stored file early.                  |

Where `<token>` is the identifier the upload returned (e.g. the `abc123...` in `/i/abc123...png`).

### `GET /`

Liveness / health check.

**Response `200 OK`**
```json
{"ok": true}
```

---

### `POST /api/upload`

Upload an image file. Send the bytes as multipart/form-data with the file in a field named `file`. An optional second field is ignored.

**Request (curl)**
```bash
curl -F "file=@photo.jpg" http://localhost:8080/api/upload
```
```javascript
// fetch
const form = new FormData();
form.append("file", fileInput.files[0]);
await fetch("/api/upload", { method: "POST", body: form });
```

**Response `200 OK`**
```json
{"url": "http://localhost:8080/i/6fb0f6ec0f314d9b8b5a6f9c1a2b3c4d.jpg"}
```
- `url` is `BASE_URL` + `/i/<token>`. If `BASE_URL` is empty, `url` is a relative path (`/i/<token>`).
- The file extension (`.jpg`, `.png`, `.webp`, etc.) is sniffed from the content bytes; a random UUID is used for the name.

**Errors**
| Status | Meaning |
|--------|---------|
| `400` | Not a valid multipart body, no `file` part, or the file is empty. |
| `413` | File exceeds `MAX_FILE_SIZE` or is truncated. |

---

### `GET /i/<token>`

Download a stored file.

**Response `200 OK`** — the original bytes, untouched.
- `Content-Type`: detected from the extension (`image/jpeg`, `image/png`, ...; `application/octet-stream` if unknown).
- `Content-Length`: file size in bytes.
- `Cache-Control`: `private, max-age=3600`.
- `X-Delete-After`: RFC 7231 HTTP-date of when the file will be auto-deleted (now + `MAX_AGE`).

**Errors**
| Status | Meaning |
|--------|---------|
| `404` | File not found (already deleted, or never existed). |
| `410` | File existed but has expired; it is removed and the URL is permanently gone. |
| `400` | Malformed token (path traversal attempts rejected). |

---

### `DELETE /i/<token>`

Delete a file before its TTL expires.

**Response `204 No Content`** on success. The URL then returns `404`.

**Errors** `404` if the file does not exist.

## Configuration (env vars)

| Variable          | Default    | Meaning                                                       |
|-------------------|------------|---------------------------------------------------------------|
| `STORAGE_DIR`     | `./img-cache` | Where files are stored. Mount a volume here for persistence.  |
| `MAX_AGE`         | `1h`       | TTL; accepts duration strings (`30m`, `1h`, `2h`) or bare seconds. |
| `BASE_URL`        | *(empty)*  | Absolute prefix used in returned URLs, e.g. `https://cdn.example.com`; empty → relative paths. |
| `MAX_FILE_SIZE`   | `500MB`    | Per-upload cap (`B`, `KB`, `MB`, `GB`, `TB`).                  |
| `MAX_TOTAL_SIZE`  | `0`        | Hard cap on total stored bytes; when hit, the oldest files are evicted until under. `0` disables. |
| `LISTEN_ADDR`     | `0.0.0.0:8080` | Bind address.                                              |

A background sweep task runs every 60s: it deletes files older than `MAX_AGE` and enforces `MAX_TOTAL_SIZE` (evicting oldest first). Total-size is also enforced on each upload.

## Run with Docker

```bash
docker build -t img-host .
docker run -d --name img-host \
  -p 8080:8080 \
  -e BASE_URL=https://your.host \
  -v $PWD/img-cache:/data \
  img-host
```

- The image is built multi-stage and runs from `scratch`: only the static binary is in the final image. The server runs as root inside the container; to harden, run with `--user` and ensure `/data` is writable by that user.
- Storage is ephemeral by design; the `-v` mount just lets you inspect/clean it.
- `scratch` has no shell — attach with `docker exec` only if you add a debug tool.

## Build locally

```bash
cargo build --release --target x86_64-unknown-linux-musl   # static, ~2 MB
cargo build --release                                      # glibc binary for local runs
```

## Notes / non-goals

- Original bytes are served untouched — no re-encode, no EXIF stripping.
- No auth, rate limiting, or HTTPS; terminate TLS at a reverse proxy.

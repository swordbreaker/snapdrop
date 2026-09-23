# teams-images

Self-hosted, throwaway image hosting. A tiny Rust service that stores an uploaded image and serves it at a URL for a short time (default one hour), then deletes it. Includes a clipboard helper for quick screenshots.

This repo has two pieces:

| Directory      | Purpose                                                        |
|----------------|-----------------------------------------------------------------|
| [`img-host/`](img-host/) | The server: an HTTP API that uploads images and serves them until they expire. Ships as a ~2 MB static Docker image. |
| [`paste-tool/`](paste-tool/) | A bash client: grabs the image from your clipboard and uploads it straight to an img-host instance. |

## Quick start

Build and run the server:

```bash
cd img-host
docker build -t img-host .
docker run -d --name img-host \
  -p 8080:8080 \
  -e BASE_URL=http://localhost:8080 \
  -v $(pwd)/../img-host-data:/data \
  img-host
```

Upload an image:

```bash
curl -F "file=@photo.png" http://localhost:8080/api/upload
# {"url":"http://localhost:8080/i/0123abc...png"}
```

The returned URL works for the configured lifetime (`MAX_AGE`, default `1h`), then the file is deleted.

## Paste an image from the clipboard

With the server running, set its base URL and upload whatever is on your clipboard:

```bash
export IMG_HOST=http://localhost:8080
./paste-tool/upload-clipboard-image.sh
# prints the URL and copies it back to the clipboard
```

Requires `curl` plus `wl-paste`/`wl-copy` (Wayland) or `xclip` (X11).

## Documentation

- **[`img-host/README.md`](img-host/README.md)** — full API reference, configuration env vars, Docker/build instructions.
- **[`paste-tool/upload-clipboard-image.sh`](paste-tool/upload-clipboard-image.sh)** — usage and environment documented in the script header.

## CI

[`.github/workflows/docker-publish.yml`](.github/workflows/docker-publish.yml) builds `img-host/` and publishes it to GHCR on pushes to `main` and on `v*` tags.

#!/usr/bin/env bash
#
# Upload the image currently in the clipboard to an img-host service.
# Prints the resulting URL and copies it back to the clipboard.
#
# Usage:
#   ./upload-clipboard-image.sh
#
# Environment:
#   IMG_HOST      Base URL of the img-host server, e.g. https://cdn.example.com
#                 (required). If empty, falls back to an interactive prompt.
#
# Dependencies (install one clipboard tool + curl):
#   X11:    xclip          (apt/pacman: xclip)
#   Wayland: wl-clipboard  (apt/pacman: wl-clipboard)
set -euo pipefail

: "${IMG_HOST:=""}"

if [[ -z "$IMG_HOST" ]]; then
    read -r -p "img-host base URL (e.g. https://cdn.example.com): " IMG_HOST
    IMG_HOST="${IMG_HOST%/}"
fi
if [[ -z "$IMG_HOST" ]]; then
    echo "error: no IMG_HOST set" >&2
    exit 1
fi

TMP="$(mktemp --suffix=.clipboard-image)"

# Grab the image out of the clipboard. Prefer a known format so the bytes are real.
if command -v wl-paste >/dev/null 2>&1; then
    wl-paste --type image/png >"$TMP" 2>/dev/null \
        || wl-paste --type image/webp >"$TMP" 2>/dev/null \
        || wl-paste --type image/jpeg >"$TMP" 2>/dev/null \
        || { echo "error: clipboard contains no image" >&2; rm -f "$TMP"; exit 1; }
elif command -v xclip >/dev/null 2>&1; then
    xclip -selection clipboard -t image/png -o >"$TMP" 2>/dev/null \
        || xclip -selection clipboard -t image/webp -o >"$TMP" 2>/dev/null \
        || xclip -selection clipboard -t image/jpeg -o >"$TMP" 2>/dev/null \
        || { echo "error: clipboard contains no image" >&2; rm -f "$TMP"; exit 1; }
else
    echo "error: need wl-clipboard (Wayland) or xclip (X11) to read the clipboard" >&2
    rm -f "$TMP"
    exit 1
fi

if [[ ! -s "$TMP" ]]; then
    echo "error: clipboard image is empty" >&2
    rm -f "$TMP"
    exit 1
fi

# Upload. The extension is sniffed by the server from the bytes, so the
# temp file's name suffix is irrelevant.
RESP="$(curl -sS -w '\n%{http_code}' -F "file=@${TMP}" "${IMG_HOST%/}/api/upload")"
HTTP_CODE="$(printf '%s' "$RESP" | tail -n1)"
BODY="$(printf '%s' "$RESP" | sed '$d')"

rm -f "$TMP"

if [[ "$HTTP_CODE" != "200" ]]; then
    echo "error: upload failed (HTTP $HTTP_CODE): $BODY" >&2
    exit 1
fi

URL="$(printf '%s' "$BODY" | sed -n 's/.*"url"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
if [[ -z "$URL" ]]; then
    echo "error: unexpected response: $BODY" >&2
    exit 1
fi

# Normalize: if the server returned a relative path, prefix the host.
case "$URL" in
    http://*|https://*) ;;
    *) URL="${IMG_HOST%/}${URL}" ;;
esac

echo "$URL"

# Put the URL back on the clipboard so it's ready to paste.
if command -v wl-copy >/dev/null 2>&1; then
    printf '%s' "$URL" | wl-copy
elif command -v xclip >/dev/null 2>&1; then
    printf '%s' "$URL" | xclip -selection clipboard
fi

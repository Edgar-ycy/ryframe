#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
config="$repo_root/deploy/nginx/ryframe.conf"
cert_dir="$(mktemp -d)"

cleanup() {
  case "$cert_dir" in
    "${TMPDIR:-/tmp}"/* | /tmp/*) rm -rf -- "$cert_dir" ;;
    *) printf 'Refusing to remove unexpected temporary path: %s\n' "$cert_dir" >&2 ;;
  esac
}
trap cleanup EXIT

openssl req -x509 -newkey rsa:2048 -nodes -days 1 \
  -subj '/CN=example.com' \
  -keyout "$cert_dir/privkey.pem" \
  -out "$cert_dir/fullchain.pem" >/dev/null 2>&1

nginx_test_output="$(
  docker run --rm \
    --volume "$config:/etc/nginx/conf.d/ryframe.conf:ro" \
    --volume "$cert_dir:/etc/letsencrypt/live/example.com:ro" \
    nginx:1.27-alpine nginx -t 2>&1
)"
printf '%s\n' "$nginx_test_output"
if grep -Eqi '(\[warn\]|deprecated)' <<<"$nginx_test_output"; then
  printf 'Nginx configuration validation emitted a warning.\n' >&2
  exit 1
fi

if grep -Eq '^[[:space:]]*listen[[:space:]]+[^;]*[[:space:]]http2([[:space:]]|;)' "$config"; then
  printf 'Nginx TLS listeners must use the standalone http2 directive.\n' >&2
  exit 1
fi
if [[ "$(grep -Ec '^[[:space:]]*http2[[:space:]]+on;' "$config")" -ne 2 ]]; then
  printf 'Each TLS server must enable HTTP/2 exactly once.\n' >&2
  exit 1
fi

grep -Eq '^[[:space:]]*limit_req_status[[:space:]]+429;' "$config"
grep -Eq '^[[:space:]]*add_header[[:space:]]+Retry-After' "$config"
if awk '
  {
    line = $0
    sub(/^[[:space:]]*/, "", line)
    if (line !~ /^#/ && line ~ /proxy_add_x_forwarded_for/) found = 1
  }
  END { exit(found ? 0 : 1) }
' "$config"; then
  printf 'Nginx must overwrite, not append, forwarded client IP headers.\n' >&2
  exit 1
fi
if grep -Eq 'location[^\n]*/uploads/' "$config"; then
  printf 'Private uploads must not be exposed through an Nginx alias.\n' >&2
  exit 1
fi

if ! awk '
  /^[[:space:]]*location[[:space:]]+=[[:space:]]+\/api\/v1\/ws[[:space:]]*\{/ { in_ws = 1; found = 1; next }
  in_ws && /^[[:space:]]*}/ { in_ws = 0; next }
  in_ws && /proxy_http_version[[:space:]]+1\.1;/ { http11 = 1 }
  in_ws && /proxy_set_header[[:space:]]+Upgrade[[:space:]]+\$http_upgrade;/ { upgrade = 1 }
  in_ws && /proxy_set_header[[:space:]]+Connection[[:space:]]+"upgrade";/ { connection = 1 }
  in_ws && /proxy_buffering[[:space:]]+off;/ { buffering = 1 }
  in_ws && /access_log[[:space:]]+off;/ { no_access_log = 1 }
  END { exit(found && http11 && upgrade && connection && buffering && no_access_log ? 0 : 1) }
' "$config"; then
  printf 'WebSocket proxy must upgrade connections, disable buffering, and suppress ticket access logs.\n' >&2
  exit 1
fi

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

docker run --rm \
  --volume "$config:/etc/nginx/conf.d/ryframe.conf:ro" \
  --volume "$cert_dir:/etc/letsencrypt/live/example.com:ro" \
  nginx:1.27-alpine nginx -t

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

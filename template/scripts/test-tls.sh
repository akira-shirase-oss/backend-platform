#!/usr/bin/env bash
# TLS to PostgreSQL with a private CA (crates/starter-db/tests/tls.rs). Makes two CAs for
# this run and a server certificate for `localhost` only, signed by the first; starts the
# stack's pinned PostgreSQL with it on 127.0.0.1 (STARTER_TLS_PG_PORT, default 61434); runs
# the ignored TLS tests against it; removes the container and the keys.
#
#   scripts/test-tls.sh
#
# Proves: sslmode=verify-full with the right root connects over TLS; a name the certificate
# does not carry, another CA and the public roots alone are refused; sslmode=require accepts
# any certificate (why the server refuses it off loopback). Needs docker and openssl.
set -euo pipefail
case "${1:-}" in -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;; esac
[ $# -eq 0 ] || { echo "test-tls.sh: takes no arguments (see --help)" >&2; exit 2; }
cd "$(dirname "$0")/.."

port="${STARTER_TLS_PG_PORT:-61434}"
image="$(sed -n 's/^ *image: *//p' deploy/compose.yaml | head -1)"
[ -n "$image" ] || { echo "test-tls: no image in deploy/compose.yaml" >&2; exit 1; }
work="$(mktemp -d)"
container="starter-tls-$$"
cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT

cat >"$work/x509.cnf" <<'EOF'
[req]
distinguished_name = dn
prompt = no
[dn]
CN = test
[ca]
basicConstraints = critical, CA:TRUE
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
[server]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = DNS:localhost
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
EOF
for ca in ca other-ca; do
  openssl req -x509 -new -nodes -newkey rsa:2048 -days 2 -config "$work/x509.cnf" \
    -extensions ca -subj "/CN=test $ca" -keyout "$work/$ca.key" -out "$work/$ca.crt" 2>/dev/null
done
openssl req -new -nodes -newkey rsa:2048 -config "$work/x509.cnf" -subj /CN=localhost \
  -keyout "$work/server.key" -out "$work/server.csr" 2>/dev/null
openssl x509 -req -in "$work/server.csr" -CA "$work/ca.crt" -CAkey "$work/ca.key" \
  -CAcreateserial -days 2 -extfile "$work/x509.cnf" -extensions server \
  -out "$work/server.crt" 2>/dev/null

# The key and certificate go in through the environment (no bind mount, so the same on
# Linux and macOS); PostgreSQL wants its key owned by it and private.
docker run -d --name "$container" -p "127.0.0.1:${port}:5432" \
  -e POSTGRES_PASSWORD=starter-tls-dev \
  -e TLS_KEY="$(cat "$work/server.key")" -e TLS_CERT="$(cat "$work/server.crt")" \
  --entrypoint bash "$image" -c '
    umask 077
    printf "%s\n" "$TLS_KEY" >/tmp/server.key
    printf "%s\n" "$TLS_CERT" >/tmp/server.crt
    chown postgres /tmp/server.key /tmp/server.crt
    exec docker-entrypoint.sh postgres -c ssl=on \
      -c ssl_cert_file=/tmp/server.crt -c ssl_key_file=/tmp/server.key' >/dev/null
ready=""
for _ in $(seq 1 120); do
  # Ready on TCP, not only on the entrypoint's temporary socket-only server.
  if docker exec "$container" pg_isready -q --host=127.0.0.1 --username=postgres 2>/dev/null; then
    ready=1
    break
  fi
  sleep 1
done
[ -n "$ready" ] || { docker logs "$container" >&2 || true; echo "test-tls: PostgreSQL did not start" >&2; exit 1; }

STARTER_TLS_TEST_PORT="$port" STARTER_TLS_TEST_CA="$work/ca.crt" \
  STARTER_TLS_TEST_OTHER_CA="$work/other-ca.crt" \
  cargo test -p starter-db --test tls --locked -- --ignored
echo "tls ok: verify-full authenticates the server; require does not, and is refused off loopback"

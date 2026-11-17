#!/usr/bin/env bash
# deploy/Caddyfile parses and adapts with the pinned Caddy release (in its official image).
set -euo pipefail
cd "$(dirname "$0")/.."
readonly IMAGE="caddy:2.11.7@sha256:36c24ad91a7ff7bc4c87fe22bfa8df3c4b18da0773d73fc12ee2c52b98c0bb7e"
if ! command -v docker >/dev/null 2>&1; then
  echo "note: docker is not available; the Caddyfile is not validated locally (CI validates it)"
  exit 0
fi
docker run --rm -e STARTER_DOMAIN=app.example.test -v "$PWD/deploy/Caddyfile:/etc/caddy/Caddyfile:ro" "$IMAGE" \
  caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile 2>&1 | tail -1

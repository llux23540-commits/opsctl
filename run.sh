#!/usr/bin/env bash
# Unified local launcher (Linux/macOS). Builds the SPA and runs the server with
# the vault auto-unsealed. For production use deploy/ (docker/k8s) instead.
#   ./run.sh              # dev run
#   ./run.sh --release    # release binary with the SPA embedded
set -euo pipefail
cd "$(dirname "$0")"

: "${OPSCTL_VAULT__PASSPHRASE:=dev-unseal-pass}"; export OPSCTL_VAULT__PASSPHRASE
: "${OPSCTL_AUTH__JWT_SECRET:=dev-jwt-secret-change-me}"; export OPSCTL_AUTH__JWT_SECRET

echo "==> building web SPA"
( cd web && [ -d node_modules ] || npm install; npm run build )

if [ "${1:-}" = "--release" ]; then
  echo "==> release build (SPA embedded) + run"
  cargo run --release -p opsctl-server
else
  echo "==> dev run at http://127.0.0.1:8443/"
  cargo run -p opsctl-server
fi

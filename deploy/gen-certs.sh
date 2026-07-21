#!/bin/sh
# Self-signed TLS cert for the opsctl edge proxy (dev / internal use).
# Production: replace deploy/certs/{fullchain.pem,privkey.pem} with real certs.
set -e
DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/certs"
mkdir -p "$DIR"
openssl req -x509 -newkey rsa:2048 -nodes -days 365 \
  -keyout "$DIR/privkey.pem" -out "$DIR/fullchain.pem" \
  -subj "/CN=opsctl.local" \
  -addext "subjectAltName=DNS:opsctl.local,DNS:localhost,IP:127.0.0.1"
chmod 600 "$DIR/privkey.pem"
echo "self-signed cert written to $DIR (CN=opsctl.local, 365d). Replace in prod."

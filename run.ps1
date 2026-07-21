# Unified local launcher (Windows). Builds the SPA and runs the server with the
# vault auto-unsealed. For production use deploy/ (docker/k8s) instead.
#
#   ./run.ps1              # dev: build web + run (debug reads web/dist from disk)
#   ./run.ps1 -Release     # build web + release binary (SPA embedded) + run
param([switch]$Release)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

# secrets: keep it simple for local; override via env for anything shared
if (-not $env:OPSCTL_VAULT__PASSPHRASE) { $env:OPSCTL_VAULT__PASSPHRASE = "dev-unseal-pass" }
if (-not $env:OPSCTL_AUTH__JWT_SECRET)  { $env:OPSCTL_AUTH__JWT_SECRET  = "dev-jwt-secret-change-me" }

Write-Host "==> building web SPA" -ForegroundColor Cyan
Push-Location web
if (-not (Test-Path node_modules)) { npm install }
npm run build
Pop-Location

if ($Release) {
  Write-Host "==> release build (SPA embedded) + run" -ForegroundColor Cyan
  cargo run --release -p opsctl-server
} else {
  Write-Host "==> dev run (vault auto-unsealed at http://127.0.0.1:8443/)" -ForegroundColor Cyan
  cargo run -p opsctl-server
}

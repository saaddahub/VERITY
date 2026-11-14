# Full check suite. A change is not done until this passes.
#   .\tools\check.ps1          build, test, clippy, fmt
#   .\tools\check.ps1 -Admin   also the admin-only integration tests (elevated terminal)
param([switch]$Admin)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

function Step($name, [scriptblock]$cmd) {
    Write-Host "==> $name" -ForegroundColor Cyan
    & $cmd
    if ($LASTEXITCODE -ne 0) { Write-Host "FAILED: $name" -ForegroundColor Red; exit 1 }
}

Step 'build'  { cargo build --workspace }
Step 'test'   { cargo test --workspace }
Step 'clippy' { cargo clippy --workspace --all-targets -- -D warnings }
Step 'fmt'    { cargo fmt --all -- --check }
if ($Admin) { Step 'admin tests' { cargo test --workspace -- --ignored --test-threads=1 } }

Write-Host 'All checks passed.' -ForegroundColor Green

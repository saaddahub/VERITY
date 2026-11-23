# M4/M5 admin checks: reinstall the service from this build, then run the
# Session Mode and Frame Story end-to-end tests. Run elevated:
#   powershell -ExecutionPolicy Bypass -File tools\m4m5-admin.ps1 [-Log <path>]
param([string]$Log = "$env:TEMP\verity-m4m5-admin.log", [string]$Filter = "")
$ErrorActionPreference = 'Continue'
Set-Location (Split-Path $PSScriptRoot -Parent)
Start-Transcript -Path $Log -Force | Out-Null

cargo build --workspace
if ($LASTEXITCODE -ne 0) { Write-Host 'FAILED: build'; Stop-Transcript; exit 1 }

$verity = '.\target\debug\verity.exe'
& $verity service stop 2>$null
& $verity service uninstall 2>$null
& $verity service install --start
if ($LASTEXITCODE -ne 0) { Write-Host 'FAILED: service install'; Stop-Transcript; exit 1 }
Start-Sleep -Seconds 3

cargo test -p verity-cli --test session_e2e -- --ignored --test-threads=1 --nocapture $Filter
$code = $LASTEXITCODE
& $verity games list
Write-Host "session_e2e exit code: $code"
Stop-Transcript | Out-Null
exit $code

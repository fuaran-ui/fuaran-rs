#Requires -Version 7.0
# fuaran-rs — Stage-0 entry point (workspace CLAUDE.md "Every new sibling ships a run.ps1").
# Full happy path: cargo fmt --check -> cargo clippy -> cargo build -> cargo test.
# Switches: -SkipFormat / -SkipBuild / -SkipTests for fast iteration.
[CmdletBinding()]
param(
    [switch]$SkipFormat,
    [switch]$SkipBuild,
    [switch]$SkipTests
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

function Resolve-Tool {
    param([string]$Name)
    $cmd = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $cmd) {
        throw "The Rust toolchain is required but '$Name' was not found on PATH. Install Rust (see Cargo.toml for the pinned edition / rust-version) and re-run."
    }
    return $cmd.Source
}

$cargo = Resolve-Tool "cargo"

if (-not $SkipFormat) {
    Write-Host "==> cargo fmt --all --check (format gate)" -ForegroundColor Cyan
    & $cargo fmt --all --check
    if ($LASTEXITCODE -ne 0) {
        throw "rustfmt found unformatted files. Run 'cargo fmt --all' before committing (workspace formatting mandate — rustfmt is the Rust analogue of Fantomas)."
    }
}

if (-not $SkipBuild) {
    Write-Host "==> cargo clippy --all-targets -- -D warnings (lint gate)" -ForegroundColor Cyan
    & $cargo clippy --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw "cargo clippy reported warnings (treated as errors)." }
    Write-Host "==> cargo build" -ForegroundColor Cyan
    & $cargo build
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed." }
}

if (-not $SkipTests) {
    Write-Host "==> cargo test" -ForegroundColor Cyan
    & $cargo test
    if ($LASTEXITCODE -ne 0) { throw "cargo test failed." }
}

Write-Host "fuaran-rs: run.ps1 complete." -ForegroundColor Green

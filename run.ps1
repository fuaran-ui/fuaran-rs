#Requires -Version 7.0
# fuaran-rs — Stage-0 entry point (workspace CLAUDE.md "Every new sibling ships a run.ps1").
# Full happy path: cargo fmt --check -> cargo clippy -> cargo build -> cargo test,
# plus a wasm32 client-module build when the target is installed.
# Switches: -SkipFormat / -SkipBuild / -SkipTests / -SkipWasm for fast iteration.
#
# Opt-in native-mobile packaging (Phase 537 — the C-ABI staticlib for the Swift /
# Kotlin surfaces):
#   -CrossTargets  build the six mobile release legs (aarch64-apple-ios{,-sim},
#                  aarch64-apple-darwin, aarch64-linux-android, armv7-linux-androideabi,
#                  x86_64-linux-android). Each leg SKIPS cleanly with a named-toolchain
#                  message when its Rust target or native toolchain (Xcode / NDK) is
#                  absent, so this stays green on a machine with no mobile toolchains.
#   -Package       assemble the Apple XCFramework (macOS-only) + the Android jniLibs/
#                  layout from the built legs (implies -CrossTargets).
#
# Opt-in driver-semantics conformance for the bounded program loop:
#   -DriverSemantics  run the loop against the program wire specification's scenario
#                     corpus on BOTH targets — natively (the `cargo test` leg, which
#                     already rides the ordinary run and reports "NOT RUN" when no
#                     corpus is present) and on `wasm32`, by building the module with
#                     the harness ABI feature and executing it under node.
#                     The corpus is resolved from FUARAN_PROGRAM_SPEC, or from a
#                     `fuaran-program-spec/` checkout beside this repository. A
#                     CLAIMED corpus that cannot be read FAILS rather than skipping.
[CmdletBinding()]
param(
    [switch]$SkipFormat,
    [switch]$SkipBuild,
    [switch]$SkipTests,
    [switch]$SkipWasm,
    [switch]$CrossTargets,
    [switch]$Package,
    [switch]$DriverSemantics
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

# --- Phase 537: native-mobile cross-target legs + packaging ------------------
# The C-ABI staticlib surface (src/ffi/, include/fuaran.h) compiles for native
# Apple + Android targets so the Swift XCFramework and Kotlin cargo-ndk .so tiers
# can link it. Each leg is OPT-IN (-CrossTargets) and skips cleanly when its Rust
# target or native toolchain is absent — no new hard dependency on the dev box.

$AppleTargets = @("aarch64-apple-ios", "aarch64-apple-ios-sim", "aarch64-apple-darwin")
$AndroidTargets = @("aarch64-linux-android", "armv7-linux-androideabi", "x86_64-linux-android")

function Test-RustTarget {
    param([string]$Target)
    $installed = & rustup target list --installed 2>$null
    return ($installed -match "^$([regex]::Escape($Target))$").Count -gt 0
}

function Invoke-CrossTargetLeg {
    param([string]$Target, [switch]$IsAndroid)

    # Apple targets link only through Xcode on macOS; skip with a named message elsewhere.
    if (($AppleTargets -contains $Target) -and (-not $IsMacOS)) {
        Write-Host "==> skip $Target — Apple targets require macOS + Xcode (build on a Mac to enable)." -ForegroundColor Yellow
        return
    }
    if (-not (Test-RustTarget $Target)) {
        Write-Host "==> skip $Target — Rust target not installed (rustup target add $Target to enable)." -ForegroundColor Yellow
        return
    }

    if ($IsAndroid) {
        # Android .so legs need the NDK linker; cargo-ndk wraps it. Skip cleanly when absent.
        # NOTE: invoke the cargo subcommand `cargo ndk` — plain `cargo build --target
        # <android-triple>` fails to find the NDK linker even with cargo-ndk installed, and
        # calling cargo-ndk.exe directly mis-parses (cargo supplies `ndk` as the first arg).
        $cargoNdk = Get-Command cargo-ndk -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $cargoNdk) {
            Write-Host "==> skip $Target — cargo-ndk not found (cargo install cargo-ndk to enable)." -ForegroundColor Yellow
            return
        }
        $ndk = $env:ANDROID_NDK_HOME
        if (-not $ndk) { $ndk = [Environment]::GetEnvironmentVariable('ANDROID_NDK_HOME', 'User') }
        if (-not $ndk) { $ndk = $env:ANDROID_NDK_ROOT }
        if (-not $ndk -or -not (Test-Path $ndk)) {
            Write-Host "==> skip $Target — ANDROID_NDK_HOME not set / not found (install the Android NDK + set ANDROID_NDK_HOME to enable)." -ForegroundColor Yellow
            return
        }
        $env:ANDROID_NDK_HOME = $ndk
        # 16KB page alignment is a 64-bit concern (arm64-v8a, x86_64) — required by Google there;
        # 32-bit armv7 uses 4KB pages. cargo-ndk (NDK r28+) defaults to 16KB on 64-bit; the flag is
        # belt-and-braces, applied to the 64-bit ABIs only.
        $prevFlags = $env:RUSTFLAGS
        if ($Target -ne "armv7-linux-androideabi") {
            $pageFlag = "-C link-arg=-Wl,-z,max-page-size=16384"
            $env:RUSTFLAGS = if ([string]::IsNullOrEmpty($prevFlags)) { $pageFlag } else { "$prevFlags $pageFlag" }
        }
        try {
            Write-Host "==> cargo ndk -t $Target build --release (.so)" -ForegroundColor Cyan
            & $cargo ndk -t $Target build --release
            if ($LASTEXITCODE -ne 0) { throw "Android cross build failed for $Target." }
        }
        finally { $env:RUSTFLAGS = $prevFlags }
    }
    else {
        Write-Host "==> cargo build --target $Target --release (staticlib)" -ForegroundColor Cyan
        & $cargo build --target $Target --release
        if ($LASTEXITCODE -ne 0) { throw "Apple cross build failed for $Target." }
    }
}

function New-AppleXcframework {
    # Assemble libfuaran_rs.a + the C header into an XCFramework (SPM binary target).
    # macOS + xcodebuild only; skips elsewhere.
    if (-not $IsMacOS) {
        Write-Host "==> skip XCFramework — assembled on macOS with xcodebuild only." -ForegroundColor Yellow
        return
    }
    $xcodebuild = Get-Command xcodebuild -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $xcodebuild) {
        Write-Host "==> skip XCFramework — xcodebuild not found (install Xcode to enable)." -ForegroundColor Yellow
        return
    }
    $out = Join-Path $PSScriptRoot "packaging/apple/Fuaran.xcframework"
    if (Test-Path $out) { Remove-Item -Recurse -Force $out }
    New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null
    $args = @("-create-xcframework")
    foreach ($t in $AppleTargets) {
        $lib = Join-Path $PSScriptRoot "target/$t/release/libfuaran_rs.a"
        if (Test-Path $lib) { $args += @("-library", $lib, "-headers", (Join-Path $PSScriptRoot "include")) }
    }
    if ($args.Count -le 1) {
        Write-Host "==> skip XCFramework — no built Apple staticlibs found (run -CrossTargets on macOS first)." -ForegroundColor Yellow
        return
    }
    $args += @("-output", $out)
    Write-Host "==> xcodebuild -create-xcframework -> $out" -ForegroundColor Cyan
    & $xcodebuild.Source @args
    if ($LASTEXITCODE -ne 0) { throw "xcframework assembly failed." }
}

function New-AndroidJniLibs {
    # Lay out the built .so files under the Android jniLibs/<abi>/ convention an AAR expects.
    $abiFor = @{
        "aarch64-linux-android"   = "arm64-v8a"
        "armv7-linux-androideabi" = "armeabi-v7a"
        "x86_64-linux-android"    = "x86_64"
    }
    $root = Join-Path $PSScriptRoot "packaging/android/jniLibs"
    $any = $false
    foreach ($t in $AndroidTargets) {
        $so = Join-Path $PSScriptRoot "target/$t/release/libfuaran_rs.so"
        if (Test-Path $so) {
            $dst = Join-Path $root $abiFor[$t]
            New-Item -ItemType Directory -Force $dst | Out-Null
            Copy-Item $so (Join-Path $dst "libfuaran_rs.so") -Force
            Write-Host "==> jniLibs/$($abiFor[$t])/libfuaran_rs.so" -ForegroundColor Cyan
            $any = $true
        }
    }
    if (-not $any) {
        Write-Host "==> skip jniLibs — no built Android .so found (run -CrossTargets with cargo-ndk first)." -ForegroundColor Yellow
    }
}

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

if (-not $SkipWasm) {
    # The browser-native client module. Build it only when the wasm32 target is
    # installed, so a machine without it is not hard-blocked (install with
    # `rustup target add wasm32-unknown-unknown` to enable this leg). The C-ABI
    # surface (src/client/wasm.rs) has no native codegen, so this is the only
    # gate that compiles it.
    $targets = & rustup target list --installed 2>$null
    if ($targets -match "wasm32-unknown-unknown") {
        Write-Host "==> cargo build --target wasm32-unknown-unknown --release (client module)" -ForegroundColor Cyan
        & $cargo build --target wasm32-unknown-unknown --release
        if ($LASTEXITCODE -ne 0) { throw "wasm32 client-module build failed." }
    } else {
        Write-Host "==> wasm32 target not installed; skipping the client-module build (rustup target add wasm32-unknown-unknown to enable)." -ForegroundColor Yellow
    }
}

if ($DriverSemantics) {
    # The bounded loop is shipped for two targets, so it is certified on two
    # targets. The native leg is an ordinary test and has already run above (it
    # reports "NOT RUN" when no corpus is present); this is the wasm32 leg,
    # which needs the harness ABI feature and a runtime to execute in.
    Write-Host "==> driver-semantics conformance (native leg)" -ForegroundColor Cyan
    & $cargo test --test driver_semantics -- --nocapture
    if ($LASTEXITCODE -ne 0) { throw "the native driver-semantics leg failed." }

    $targets = & rustup target list --installed 2>$null
    if ($targets -match "wasm32-unknown-unknown") {
        $node = Get-Command node -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $node) {
            Write-Host "==> skip the wasm32 driver-semantics leg — node not found on PATH (install Node.js to enable)." -ForegroundColor Yellow
        }
        else {
            Write-Host "==> cargo build --target wasm32-unknown-unknown --release --features driver-semantics-abi" -ForegroundColor Cyan
            & $cargo build --target wasm32-unknown-unknown --release --features driver-semantics-abi
            if ($LASTEXITCODE -ne 0) { throw "the wasm32 harness-ABI build failed." }
            Write-Host "==> driver-semantics conformance (wasm32 leg)" -ForegroundColor Cyan
            & $node.Source (Join-Path $PSScriptRoot "js/driver-semantics.mjs")
            if ($LASTEXITCODE -ne 0) { throw "the wasm32 driver-semantics leg failed." }
        }
    }
    else {
        Write-Host "==> skip the wasm32 driver-semantics leg — Rust target not installed (rustup target add wasm32-unknown-unknown to enable)." -ForegroundColor Yellow
    }
}

if ($CrossTargets -or $Package) {
    Write-Host "==> native-mobile cross-target legs (Phase 537; each skips cleanly when its toolchain is absent)" -ForegroundColor Cyan
    foreach ($t in $AppleTargets) { Invoke-CrossTargetLeg -Target $t }
    foreach ($t in $AndroidTargets) { Invoke-CrossTargetLeg -Target $t -IsAndroid }
}

if ($Package) {
    Write-Host "==> packaging (Apple XCFramework + Android jniLibs; skips on the wrong host)" -ForegroundColor Cyan
    New-AppleXcframework
    New-AndroidJniLibs
}

Write-Host "fuaran-rs: run.ps1 complete." -ForegroundColor Green

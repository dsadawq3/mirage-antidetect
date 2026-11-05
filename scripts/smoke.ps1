#!/usr/bin/env pwsh
# Mirage smoke test: rustfmt check, workspace build, workspace tests.
# Runs every step even if a previous one failed, prints a per-step verdict
# and a final summary.
#
# Exit codes:
#   0 - all steps passed
#   1 - at least one step failed
#   2 - environment problem (cargo not found / not a Mirage checkout)

$ErrorActionPreference = 'Stop'

# The repository root is the parent of the directory holding this script.
$RepoRoot = Split-Path -Parent $PSScriptRoot

# Preflight: cargo must be available and this must be a workspace checkout.

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host 'ERROR: cargo not found in PATH.'
    Write-Host 'Install the Rust toolchain first, then re-run this script:'
    Write-Host '  winget install --id Rustlang.Rustup   (then restart the shell)'
    Write-Host '  or download rustup-init from https://rustup.rs'
    exit 2
}

$WorkspaceManifest = Join-Path $RepoRoot 'Cargo.toml'
if (-not (Test-Path -LiteralPath $WorkspaceManifest -PathType Leaf)) {
    Write-Host "ERROR: Cargo.toml not found at $WorkspaceManifest"
    Write-Host 'This script must stay inside the scripts/ directory of the Mirage repository.'
    exit 2
}

Push-Location -LiteralPath $RepoRoot

$StepsTotal = 3
$StepsPassed = 0
$StepsFailed = 0
$FailedSteps = New-Object System.Collections.Generic.List[string]

# Step 1: rustfmt check

try {
    Write-Host 'Step 1/3: cargo fmt --all -- --check'
    cargo fmt --all -- --check
    $Code = $LASTEXITCODE
    if ($Code -ne 0) {
        throw "cargo fmt exited with code $Code"
    }
    Write-Host 'Step 1/3: PASSED'
    $StepsPassed++
}
catch {
    $StepsFailed++
    [void]$FailedSteps.Add('fmt')
    Write-Host "Step 1/3: FAILED: $($_.Exception.Message)"
    Write-Host '  Formatting does not match rustfmt.toml, or the rustfmt component is missing.'
    Write-Host '  Fix: cargo fmt --all'
    Write-Host '  Component (if missing): rustup component add rustfmt'
}

# Step 2: workspace build

try {
    Write-Host 'Step 2/3: cargo build --workspace'
    cargo build --workspace
    $Code = $LASTEXITCODE
    if ($Code -ne 0) {
        throw "cargo build exited with code $Code"
    }
    Write-Host 'Step 2/3: PASSED'
    $StepsPassed++
}
catch {
    $StepsFailed++
    [void]$FailedSteps.Add('build')
    Write-Host "Step 2/3: FAILED: $($_.Exception.Message)"
    Write-Host '  The workspace failed to compile; read the compiler errors above.'
    Write-Host '  If workspace members are missing (crates/*/Cargo.toml), the root'
    Write-Host '  Cargo.toml cannot be loaded and every cargo command will fail.'
}

# Step 3: workspace tests

try {
    Write-Host 'Step 3/3: cargo test --workspace'
    cargo test --workspace
    $Code = $LASTEXITCODE
    if ($Code -ne 0) {
        throw "cargo test exited with code $Code"
    }
    Write-Host 'Step 3/3: PASSED'
    $StepsPassed++
}
catch {
    $StepsFailed++
    [void]$FailedSteps.Add('test')
    Write-Host "Step 3/3: FAILED: $($_.Exception.Message)"
    Write-Host '  Tests failed or the workspace failed to compile; read the output above.'
}

# Summary

Pop-Location

Write-Host ''
Write-Host "Summary: $StepsPassed of $StepsTotal steps passed."
if ($StepsFailed -gt 0) {
    Write-Host "Failed steps: $($FailedSteps -join ', ')"
    exit 1
}

exit 0

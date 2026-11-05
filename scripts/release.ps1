#!/usr/bin/env pwsh
# Mirage release pipeline: git cleanliness -> fmt -> clippy -> tests -> release
# build -> packaging. PowerShell 7, no admin rights required.
#
# Like scripts/smoke.ps1, every step runs even if a previous one failed, with a
# per-step verdict and a final summary. The packaging step always runs last so
# a failed pipeline does not leave a stale zip unnoticed (the summary says
# which steps failed; fix them and re-run).
#
# Steps:
#   1. git working tree check (advisory: uncommitted files warn but never
#      block; no git repository / no git binary - info, not an error)
#   2. cargo fmt --all -- --check
#   3. cargo clippy --workspace --all-targets -- -D warnings
#   4. cargo test --workspace
#   5. cargo build --release --workspace
#   6. scripts/package.ps1 (child process; builds dist\mirage-<version>-win-x64.zip)
#
# Exit codes:
#   0 - all steps passed
#   1 - at least one step failed
#   2 - environment problem (cargo not found / not a Mirage checkout)

$ErrorActionPreference = 'Stop'

# Native command stderr must not become terminating errors (PS 7.3+;
# harmless no-op elsewhere).
$PSNativeCommandUseErrorActionPreference = $false

# The repository root is the parent of the directory holding this script.
$RepoRoot = Split-Path -Parent $PSScriptRoot

if ($PSEdition -ne 'Core') {
    Write-Host 'WARN: this pipeline targets PowerShell 7 (pwsh); continuing on Windows PowerShell.'
    Write-Host '  If anything misbehaves, install PowerShell 7:  winget install Microsoft.PowerShell'
}

# --- Preflight ----------------------------------------------------------------

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

$StepsTotal = 6
$StepsPassed = 0
$StepsFailed = 0
$FailedSteps = New-Object System.Collections.Generic.List[string]

Push-Location -LiteralPath $RepoRoot
try {

    # Step 1: git working tree check (advisory)

    try {
        $StepStarted = [System.Diagnostics.Stopwatch]::StartNew()
        Write-Host 'Step 1/6: git working tree check'

        $GitFound = [bool](Get-Command git -ErrorAction SilentlyContinue)
        $IsRepo = $false
        $StatusOk = $false
        $Dirty = @()
        if ($GitFound) {
            # Relax EAP: probing a non-repo makes git write to stderr, which
            # must not throw under $ErrorActionPreference = 'Stop'.
            $PreviousEap = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            try {
                $null = git -C $RepoRoot rev-parse --is-inside-work-tree 2>$null
                $IsRepo = ($LASTEXITCODE -eq 0)
                if ($IsRepo) {
                    $Dirty = @(git -C $RepoRoot status --porcelain=v1 2>$null)
                    $StatusOk = ($LASTEXITCODE -eq 0)
                }
            }
            finally {
                $ErrorActionPreference = $PreviousEap
            }
        }

        if (-not $GitFound) {
            Write-Host '  INFO: git not found in PATH - cleanliness check skipped.'
        }
        elseif (-not $IsRepo) {
            Write-Host '  INFO: not a git repository - cleanliness check skipped.'
        }
        elseif (-not $StatusOk) {
            Write-Host '  WARN: git status failed to run - cleanliness check inconclusive (not blocking).'
        }
        elseif ($Dirty.Count -gt 0) {
            Write-Host "  WARN: working tree is not clean ($($Dirty.Count) uncommitted entries) - NOT blocking:"
            foreach ($Line in ($Dirty | Select-Object -First 10)) {
                Write-Host "    $Line"
            }
            if ($Dirty.Count -gt 10) {
                Write-Host "    ... and $($Dirty.Count - 10) more"
            }
            Write-Host '  Commit or stash before a real release: the package may capture uncommitted files.'
        }
        else {
            Write-Host '  git: working tree clean.'
        }
        $StepStarted.Stop()
        Write-Host ('Step 1/6: PASSED ({0:mm\:ss})' -f $StepStarted.Elapsed)
        $StepsPassed++
    }
    catch {
        $StepsFailed++
        [void]$FailedSteps.Add('git-check')
        Write-Host "Step 1/6: FAILED: $($_.Exception.Message)"
        Write-Host '  Unexpected git failure; the check is advisory, but investigate before releasing.'
    }

    # Step 2: rustfmt check

    try {
        $StepStarted = [System.Diagnostics.Stopwatch]::StartNew()
        Write-Host 'Step 2/6: cargo fmt --all -- --check'
        cargo fmt --all -- --check
        $Code = $LASTEXITCODE
        if ($Code -ne 0) {
            throw "cargo fmt exited with code $Code"
        }
        $StepStarted.Stop()
        Write-Host ('Step 2/6: PASSED ({0:mm\:ss})' -f $StepStarted.Elapsed)
        $StepsPassed++
    }
    catch {
        $StepsFailed++
        [void]$FailedSteps.Add('fmt')
        Write-Host "Step 2/6: FAILED: $($_.Exception.Message)"
        Write-Host '  Formatting does not match rustfmt.toml, or the rustfmt component is missing.'
        Write-Host '  Fix: cargo fmt --all'
        Write-Host '  Component (if missing): rustup component add rustfmt'
    }

    # Step 3: clippy (strict: any lint is an error)

    try {
        $StepStarted = [System.Diagnostics.Stopwatch]::StartNew()
        Write-Host 'Step 3/6: cargo clippy --workspace --all-targets -- -D warnings'
        cargo clippy --workspace --all-targets -- -D warnings
        $Code = $LASTEXITCODE
        if ($Code -ne 0) {
            throw "cargo clippy exited with code $Code"
        }
        $StepStarted.Stop()
        Write-Host ('Step 3/6: PASSED ({0:mm\:ss})' -f $StepStarted.Elapsed)
        $StepsPassed++
    }
    catch {
        $StepsFailed++
        [void]$FailedSteps.Add('clippy')
        Write-Host "Step 3/6: FAILED: $($_.Exception.Message)"
        Write-Host '  Clippy reported warnings (-D warnings escalates every lint to an error),'
        Write-Host '  or the clippy component is missing, or the workspace failed to compile.'
        Write-Host '  Fix the lints above, then re-run.'
        Write-Host '  Component (if missing): rustup component add clippy'
    }

    # Step 4: workspace tests

    try {
        $StepStarted = [System.Diagnostics.Stopwatch]::StartNew()
        Write-Host 'Step 4/6: cargo test --workspace'
        cargo test --workspace
        $Code = $LASTEXITCODE
        if ($Code -ne 0) {
            throw "cargo test exited with code $Code"
        }
        $StepStarted.Stop()
        Write-Host ('Step 4/6: PASSED ({0:mm\:ss})' -f $StepStarted.Elapsed)
        $StepsPassed++
    }
    catch {
        $StepsFailed++
        [void]$FailedSteps.Add('test')
        Write-Host "Step 4/6: FAILED: $($_.Exception.Message)"
        Write-Host '  Tests failed or the workspace failed to compile; read the output above.'
        Write-Host '  The suite is offline: sockets bind to localhost only, no Chrome is spawned.'
    }

    # Step 5: release build

    try {
        $StepStarted = [System.Diagnostics.Stopwatch]::StartNew()
        Write-Host 'Step 5/6: cargo build --release --workspace'
        cargo build --release --workspace
        $Code = $LASTEXITCODE
        if ($Code -ne 0) {
            throw "cargo build exited with code $Code"
        }
        $StepStarted.Stop()
        Write-Host ('Step 5/6: PASSED ({0:mm\:ss})' -f $StepStarted.Elapsed)
        $StepsPassed++
    }
    catch {
        $StepsFailed++
        [void]$FailedSteps.Add('build-release')
        Write-Host "Step 5/6: FAILED: $($_.Exception.Message)"
        Write-Host '  The release profile (thin LTO, codegen-units = 1) failed to compile;'
        Write-Host '  read the compiler errors above.'
    }

    # Step 6: packaging

    try {
        $StepStarted = [System.Diagnostics.Stopwatch]::StartNew()
        Write-Host 'Step 6/6: scripts/package.ps1'

        $PackageScript = Join-Path $PSScriptRoot 'package.ps1'
        if (-not (Test-Path -LiteralPath $PackageScript -PathType Leaf)) {
            throw "package.ps1 not found at $PackageScript"
        }

        # Child process on purpose: package.ps1 exits with its own code and an
        # in-process call would terminate this pipeline instead of its step.
        $HostExe = (Get-Command pwsh -ErrorAction SilentlyContinue).Source
        if (-not $HostExe) {
            $HostExe = (Get-Command powershell -ErrorAction SilentlyContinue).Source
        }
        if (-not $HostExe) {
            throw 'no PowerShell host executable (pwsh / powershell) found to run package.ps1'
        }

        & $HostExe -NoProfile -ExecutionPolicy Bypass -File $PackageScript
        $Code = $LASTEXITCODE
        if ($Code -ne 0) {
            throw "package.ps1 exited with code $Code (see its output above)"
        }
        $StepStarted.Stop()
        Write-Host ('Step 6/6: PASSED ({0:mm\:ss})' -f $StepStarted.Elapsed)
        $StepsPassed++
    }
    catch {
        $StepsFailed++
        [void]$FailedSteps.Add('package')
        Write-Host "Step 6/6: FAILED: $($_.Exception.Message)"
        Write-Host '  The packager prints the exact reason above (missing input, bad version, IO error).'
    }
}
finally {
    Pop-Location
}

# --- Summary ------------------------------------------------------------------

Write-Host ''
Write-Host "Summary: $StepsPassed of $StepsTotal steps passed."
if ($StepsFailed -gt 0) {
    Write-Host "Failed steps: $($FailedSteps -join ', ')"
    Write-Host 'Release aborted: fix the failures above and re-run.'
    exit 1
}

$DistDir = Join-Path $RepoRoot 'dist'
$Zip = Get-ChildItem -LiteralPath $DistDir -Filter 'mirage-*-win-x64.zip' -File -ErrorAction SilentlyContinue |
    Sort-Object -Property LastWriteTime -Descending | Select-Object -First 1
if ($Zip) {
    Write-Host "Artifact: $($Zip.FullName)"
}
Write-Host 'Release pipeline OK.'
exit 0

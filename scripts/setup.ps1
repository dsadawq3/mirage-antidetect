#!/usr/bin/env pwsh
# Mirage development environment check (Windows, no admin rights required).
# Verifies the Rust toolchain and Google Chrome, then creates the Mirage
# data directories under %LOCALAPPDATA%\Mirage if they do not exist yet.
#
# Exit codes:
#   0 - environment is ready
#   1 - something critical is missing (see the summary)

$ErrorActionPreference = 'Stop'

# The repository root is the parent of the directory holding this script.
# Rust checks run from there so that rust-toolchain.toml selects the same
# toolchain Mirage actually builds with.
$RepoRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $RepoRoot

$Ok = New-Object System.Collections.Generic.List[string]
$Missing = New-Object System.Collections.Generic.List[string]
$Warn = New-Object System.Collections.Generic.List[string]

if ($PSVersionTable.PSVersion.Major -lt 7) {
    $Warn.Add("Running under PowerShell $($PSVersionTable.PSVersion); PowerShell 7 or newer is recommended (winget install --id Microsoft.PowerShell).")
}

# 1. Rust toolchain: rustc + cargo + rustfmt/clippy components

$CargoCmd = Get-Command cargo -ErrorAction SilentlyContinue
$RustcCmd = Get-Command rustc -ErrorAction SilentlyContinue
$RustupCmd = Get-Command rustup -ErrorAction SilentlyContinue

if ($null -eq $CargoCmd -or $null -eq $RustcCmd) {
    $Missing.Add('Rust toolchain (rustc and cargo) not found in PATH.')
    $Missing.Add('  Install: winget install --id Rustlang.Rustup   (then restart the shell)')
    $Missing.Add('  or:      download and run rustup-init.exe from https://rustup.rs')
    $Missing.Add('  then:    rustup default stable')
}
else {
    # NOTE: no `| Select-Object -First 1` here - on PowerShell 7.4+ it stops
    # the pipeline early and $LASTEXITCODE never gets set for native commands.
    $RustcOutput = @(rustc --version)
    $RustcExit = $LASTEXITCODE
    $RustcVersion = if ($RustcOutput.Count -gt 0) { [string]$RustcOutput[0] } else { $null }
    $CargoOutput = @(cargo --version)
    $CargoExit = $LASTEXITCODE
    $CargoVersion = if ($CargoOutput.Count -gt 0) { [string]$CargoOutput[0] } else { $null }

    if ($RustcExit -ne 0 -or [string]::IsNullOrWhiteSpace($RustcVersion)) {
        $Missing.Add("rustc found at $($RustcCmd.Source) but failed to run (exit code $RustcExit).")
        $Missing.Add('  Check the toolchain pinned by rust-toolchain.toml: rustup show')
        $Missing.Add('  If it is not installed: rustup toolchain install')
        $Missing.Add('  If no default toolchain is configured: rustup default stable')
    }
    else {
        $Ok.Add("rustc: $RustcVersion")
    }

    if ($CargoExit -ne 0 -or [string]::IsNullOrWhiteSpace([string]$CargoVersion)) {
        $Missing.Add("cargo found at $($CargoCmd.Source) but failed to run (exit code $CargoExit).")
        $Missing.Add('  Check the toolchain pinned by rust-toolchain.toml: rustup show')
    }
    else {
        $Ok.Add("cargo: $CargoVersion")
    }

    # rustfmt and clippy are required by scripts/smoke.ps1 and CI.
    # Component names may be bare ("rustfmt") or target-qualified
    # ("rustfmt-x86_64-pc-windows-msvc"), depending on the rustup version.
    if ($null -ne $RustupCmd) {
        $Components = @(rustup component list --installed)
        if ($LASTEXITCODE -eq 0 -and $Components.Count -gt 0) {
            $HasRustfmt = [bool]($Components | Where-Object { $_ -eq 'rustfmt' -or $_ -like 'rustfmt-*' })
            $HasClippy = [bool]($Components | Where-Object { $_ -eq 'clippy' -or $_ -like 'clippy-*' })
            if ($HasRustfmt) {
                $Ok.Add('rustfmt: installed')
            }
            else {
                $Warn.Add('rustfmt component is missing. Fix: rustup component add rustfmt (required by cargo fmt in smoke tests and CI).')
            }
            if ($HasClippy) {
                $Ok.Add('clippy: installed')
            }
            else {
                $Warn.Add('clippy component is missing. Fix: rustup component add clippy (required by cargo clippy in CI).')
            }
        }
        else {
            $Warn.Add('Could not determine installed rustup components (no default toolchain configured?). Fix: rustup default stable')
        }
    }
    else {
        $Warn.Add('rustup not found; cannot verify rustfmt/clippy components (system Rust install?).')
    }
}

# 2. Google Chrome: the standard install locations, same set the launcher probes

$ChromeCandidates = New-Object System.Collections.Generic.List[string]
if ($env:ProgramFiles) {
    $ChromeCandidates.Add((Join-Path $env:ProgramFiles 'Google\Chrome\Application\chrome.exe'))
}
if (${env:ProgramFiles(x86)}) {
    $ChromeCandidates.Add((Join-Path ${env:ProgramFiles(x86)} 'Google\Chrome\Application\chrome.exe'))
}
if ($env:LOCALAPPDATA) {
    $ChromeCandidates.Add((Join-Path $env:LOCALAPPDATA 'Google\Chrome\Application\chrome.exe'))
}

$ChromePath = $null
foreach ($Candidate in $ChromeCandidates) {
    if (Test-Path -LiteralPath $Candidate -PathType Leaf) {
        $ChromePath = $Candidate
        break
    }
}

# Fallback: the App Paths registry keys written by the Chrome installer.
if ($null -eq $ChromePath) {
    $RegistryKeys = @(
        'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\chrome.exe',
        'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\chrome.exe',
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths\chrome.exe'
    )
    foreach ($Key in $RegistryKeys) {
        if (Test-Path -LiteralPath $Key) {
            $Registered = (Get-ItemProperty -LiteralPath $Key -ErrorAction SilentlyContinue).'(default)'
            if ($Registered) {
                $Registered = [string]$Registered
                $Registered = $Registered.Trim().Trim('"')
                if ($Registered -and (Test-Path -LiteralPath $Registered -PathType Leaf)) {
                    $ChromePath = $Registered
                    break
                }
            }
        }
    }
}

if ($null -ne $ChromePath) {
    $ChromeReport = "Chrome: $ChromePath"
    $ChromeVersion = $null
    try {
        $ChromeVersion = (Get-Item -LiteralPath $ChromePath).VersionInfo.ProductVersion
    }
    catch {
        # Version lookup is best-effort; ignore failures.
    }
    if ($ChromeVersion) {
        $ChromeReport += " (version $ChromeVersion)"
    }
    $Ok.Add($ChromeReport)

    # Mirage fingerprint data targets Chrome 131 (see docs/CONTRACT.md).
    if ($ChromeVersion -match '^(\d+)\.') {
        $ChromeMajor = [int]$Matches[1]
        if ($ChromeMajor -ne 131) {
            $Warn.Add("Chrome major version is $ChromeMajor, but Mirage fingerprint data targets Chrome 131 (docs/CONTRACT.md). The launcher will still run this Chrome, but a version mismatch increases detection risk.")
        }
    }
}
else {
    $Missing.Add('Google Chrome not found in the standard locations:')
    foreach ($Candidate in $ChromeCandidates) {
        $Missing.Add("  checked: $Candidate")
    }
    $Missing.Add('  Install: winget install --id Google.Chrome')
    $Missing.Add('  or:      download from https://www.google.com/chrome/')
}

# 3. Mirage data directories: %LOCALAPPDATA%\Mirage\vault and \profiles

if (-not $env:LOCALAPPDATA) {
    $Missing.Add('LOCALAPPDATA is not set; cannot locate the Mirage data directory.')
}
else {
    $MirageRoot = Join-Path $env:LOCALAPPDATA 'Mirage'
    $DataDirs = @(
        (Join-Path $MirageRoot 'vault'),
        (Join-Path $MirageRoot 'profiles')
    )
    foreach ($Dir in $DataDirs) {
        if (Test-Path -LiteralPath $Dir -PathType Container) {
            $Ok.Add("Directory exists: $Dir")
        }
        else {
            try {
                New-Item -ItemType Directory -Path $Dir -Force | Out-Null
                $Ok.Add("Directory created: $Dir")
            }
            catch {
                $Missing.Add("Failed to create directory $Dir : $($_.Exception.Message)")
            }
        }
    }
}

# Summary

Pop-Location

Write-Host ''
Write-Host 'Mirage development environment check'
Write-Host ''

if ($Ok.Count -gt 0) {
    Write-Host 'OK:'
    foreach ($Item in $Ok) { Write-Host "  $Item" }
    Write-Host ''
}

if ($Warn.Count -gt 0) {
    Write-Host 'Warnings:'
    foreach ($Item in $Warn) { Write-Host "  $Item" }
    Write-Host ''
}

if ($Missing.Count -gt 0) {
    Write-Host 'Missing (fix before developing):'
    foreach ($Item in $Missing) { Write-Host "  $Item" }
    Write-Host ''
    Write-Host 'Result: environment is NOT ready. Follow the steps listed above.'
    exit 1
}

Write-Host 'Result: environment is ready for Mirage development.'
exit 0

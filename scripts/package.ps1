#!/usr/bin/env pwsh
# Mirage release packaging: builds dist\mirage-<version>-win-x64.zip.
# The version is parsed from [workspace.package] in the root Cargo.toml with a
# plain line-by-line regex scan (no TOML parser dependency).
#
# Archive layout (single top-level folder mirage-<version>-win-x64\):
#   bin\*.exe        crate binaries from target\release (examples excluded)
#   README.md
#   LICENSE          MIT
#   docs\            full documentation, CONTRACT.md included (user-facing)
#   inject\          JS spoof layer (kept in the zip for MIRAGE_INJECT_DIR-style
#                    overrides and offline inspection)
#   fp-db\README.md  fingerprint database notes (JSON bases stay in the repo)
#
# Exit codes:
#   0 - archive built
#   1 - packaging failed (required input missing, copy/compress error)
#   2 - environment problem (Cargo.toml missing, version unparsable)

$ErrorActionPreference = 'Stop'

# The repository root is the parent of the directory holding this script.
$RepoRoot = Split-Path -Parent $PSScriptRoot

function Format-MirageSize {
    param([Parameter(Mandatory)][long]$Bytes)
    if ($Bytes -ge 1MB) { return ('{0:N2} MiB' -f ($Bytes / 1MB)) }
    if ($Bytes -ge 1KB) { return ('{0:N1} KiB' -f ($Bytes / 1KB)) }
    return "$Bytes B"
}

# --- Preflight: the workspace manifest must exist ----------------------------

$ManifestPath = Join-Path $RepoRoot 'Cargo.toml'
if (-not (Test-Path -LiteralPath $ManifestPath -PathType Leaf)) {
    Write-Host "ERROR: Cargo.toml not found at $ManifestPath"
    Write-Host 'This script must stay inside the scripts/ directory of the Mirage repository.'
    exit 2
}

# --- Version: [workspace.package] version from the root manifest -------------

$Version = $null
$InSection = $false
foreach ($Line in (Get-Content -LiteralPath $ManifestPath)) {
    if ($Line -match '^\s*\[([^\]]+)\]') {
        $InSection = ($Matches[1] -eq 'workspace.package')
        continue
    }
    if ($InSection -and $Line -match '^\s*version\s*=\s*"([^"]+)"') {
        $Version = $Matches[1]
        break
    }
}

if (-not $Version) {
    Write-Host "ERROR: no version string found in the [workspace.package] section of $ManifestPath"
    Write-Host 'Expected a line like:  version = "0.1.0"'
    exit 2
}
if ($Version -notmatch '^\d+\.\d+\.\d+') {
    Write-Host "ERROR: unparsable version string '$Version' in [workspace.package] of $ManifestPath"
    Write-Host 'Expected the X.Y.Z form.'
    exit 2
}

$PackageName = "mirage-$Version-win-x64"
Write-Host "Mirage packaging: version $Version (from [workspace.package] in Cargo.toml)"

# --- Binaries: crate executables from target\release -------------------------
# Only the release root is scanned: crate binaries land there, while cargo puts
# examples into target\release\examples and build-script artifacts into
# target\release\build — neither belongs in the package.

$ReleaseDir = Join-Path $RepoRoot 'target\release'
$Exes = @()
if (Test-Path -LiteralPath $ReleaseDir -PathType Container) {
    $Exes = @(Get-ChildItem -LiteralPath $ReleaseDir -Filter '*.exe' -File -ErrorAction SilentlyContinue)
}

if ($Exes.Count -gt 0) {
    Write-Host "Binaries: $($Exes.Count) exe in target\release:"
    foreach ($Exe in $Exes) {
        Write-Host "  $($Exe.Name) ($(Format-MirageSize $Exe.Length))"
    }
}
else {
    Write-Host 'WARN: no crate binaries (*.exe) found in target\release.'
    Write-Host '  The wave-1 workspace is library-only (no [[bin]] targets), so this is expected.'
    Write-Host '  Building the archive without a bin\ folder. If binaries were expected, run first:'
    Write-Host '    cargo build --release --workspace'
}

# --- Required static inputs ---------------------------------------------------

foreach ($Required in @('README.md', 'LICENSE', 'docs', 'inject')) {
    $Path = Join-Path $RepoRoot $Required
    if (-not (Test-Path -LiteralPath $Path)) {
        Write-Host "ERROR: required package input missing: $Required (expected at $Path)"
        exit 1
    }
}
$FpDbReadme = Join-Path $RepoRoot 'fp-db\README.md'
if (-not (Test-Path -LiteralPath $FpDbReadme -PathType Leaf)) {
    Write-Host "ERROR: required package input missing: fp-db\README.md (expected at $FpDbReadme)"
    exit 1
}

# --- Staging and compression --------------------------------------------------

$DistDir = Join-Path $RepoRoot 'dist'
$StageRoot = Join-Path $DistDir '.staging'
$Stage = Join-Path $StageRoot $PackageName
$ZipPath = Join-Path $DistDir "$PackageName.zip"

try {
    if (Test-Path -LiteralPath $Stage) {
        Remove-Item -LiteralPath $Stage -Recurse -Force
    }
    New-Item -ItemType Directory -Path $Stage -Force | Out-Null

    Write-Host "Staging $Stage ..."
    if ($Exes.Count -gt 0) {
        New-Item -ItemType Directory -Path (Join-Path $Stage 'bin') -Force | Out-Null
        foreach ($Exe in $Exes) {
            Copy-Item -LiteralPath $Exe.FullName -Destination (Join-Path $Stage 'bin') -Force
        }
        Write-Host '  + bin\ (crate binaries from target\release)'
    }
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'README.md') -Destination $Stage -Force
    Write-Host '  + README.md'
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'LICENSE') -Destination $Stage -Force
    Write-Host '  + LICENSE'
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'docs') -Destination $Stage -Recurse -Force
    Write-Host '  + docs\ (incl. CONTRACT.md — user-facing documentation)'
    Copy-Item -LiteralPath (Join-Path $RepoRoot 'inject') -Destination $Stage -Recurse -Force
    Write-Host '  + inject\ (JS spoof layer, for MIRAGE_INJECT_DIR-style overrides)'
    New-Item -ItemType Directory -Path (Join-Path $Stage 'fp-db') -Force | Out-Null
    Copy-Item -LiteralPath $FpDbReadme -Destination (Join-Path $Stage 'fp-db') -Force
    Write-Host '  + fp-db\README.md'

    Compress-Archive -LiteralPath $Stage -DestinationPath $ZipPath -Force
}
finally {
    if (Test-Path -LiteralPath $StageRoot) {
        Remove-Item -LiteralPath $StageRoot -Recurse -Force
    }
}

# --- Report: contents and size ------------------------------------------------

$ZipItem = Get-Item -LiteralPath $ZipPath
Add-Type -AssemblyName System.IO.Compression.FileSystem
$Archive = [System.IO.Compression.ZipFile]::OpenRead($ZipPath)
try {
    $TotalUncompressed = ($Archive.Entries | Measure-Object -Property Length -Sum).Sum
    Write-Host ''
    Write-Host "Archive: $ZipPath"
    Write-Host "Contents ($($Archive.Entries.Count) entries, $(Format-MirageSize $TotalUncompressed) uncompressed):"
    foreach ($Entry in ($Archive.Entries | Sort-Object -Property FullName)) {
        if ($Entry.FullName -match '[\\/]$') { continue }   # directory entries, if any
        $Name = $Entry.FullName -replace '\\', '/'          # Compress-Archive writes '\' separators
        Write-Host ('  {0,-60} {1,10}' -f $Name, (Format-MirageSize $Entry.Length))
    }
}
finally {
    $Archive.Dispose()
}
Write-Host "Archive size on disk: $(Format-MirageSize $ZipItem.Length)"
Write-Host "Done: $PackageName.zip"
exit 0

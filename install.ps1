# icm installer for Windows — https://github.com/rtk-ai/icm
#
# Usage:
#   irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex
#
# Re-run to upgrade. Pass arguments by downloading first:
#   $script = irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1
#   & ([ScriptBlock]::Create($script)) -Version "icm-v0.10.28"
#
# Every download is verified against the release's checksums.txt (SHA256).

param(
    [string]$Version = "",
    [string]$InstallDir = ""
)

$ErrorActionPreference = "Stop"

$Repo = "rtk-ai/icm"
$BinaryName = "icm"

function Get-Arch {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch) {
        "X64"   { return "x86_64" }
        default { throw "Unsupported architecture: $arch. Only x86_64 is supported on Windows." }
    }
}

function Get-LatestVersion {
    $release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
    return $release.tag_name
}

function Get-CurrentVersion {
    param([string]$BinaryPath)
    if (Test-Path $BinaryPath) {
        try {
            $output = & $BinaryPath --version 2>$null
            if ($output -match '\s+(\S+)$') {
                return $Matches[1]
            }
        } catch { }
    }
    return $null
}

# Parse "<sha256>  <filename>" lines from checksums.txt.
function Get-ExpectedSha {
    param([string]$ChecksumsPath, [string]$Filename)
    foreach ($line in Get-Content $ChecksumsPath) {
        $parts = $line.Trim() -split '\s+', 2
        if ($parts.Length -eq 2 -and $parts[1] -eq $Filename) {
            return $parts[0].ToLower()
        }
    }
    throw "No checksum entry for $Filename in checksums.txt"
}

$Arch = Get-Arch
if (-not $Version) {
    Write-Host "[INFO] Fetching latest release..." -ForegroundColor Green
    $Version = Get-LatestVersion
}
$Target = "$Arch-pc-windows-msvc"
if (-not $InstallDir) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "icm\bin"
}
$BinaryPath = Join-Path $InstallDir "$BinaryName.exe"

$PreviousVersion = Get-CurrentVersion -BinaryPath $BinaryPath
if ($PreviousVersion) {
    Write-Host "[INFO] Upgrading icm $PreviousVersion -> $Version ($Arch)" -ForegroundColor Green
} else {
    Write-Host "[INFO] Installing icm $Version ($Arch)" -ForegroundColor Green
}

# Create install directory
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null

# Download binary archive
$ArchiveName = "$BinaryName-$Target.zip"
$BaseUrl = "https://github.com/$Repo/releases/download/$Version"
$Url = "$BaseUrl/$ArchiveName"
$TempZip = Join-Path $env:TEMP "$BinaryName-$([guid]::NewGuid()).zip"
$TempChecksums = Join-Path $env:TEMP "icm-checksums-$([guid]::NewGuid()).txt"
$TempDir = Join-Path $env:TEMP "$BinaryName-extract-$([guid]::NewGuid())"

try {
    Write-Host "[INFO] Downloading $ArchiveName" -ForegroundColor Green
    Invoke-WebRequest -Uri $Url -OutFile $TempZip -UseBasicParsing

    # SHA256 verification — mandatory, never skipped.
    Write-Host "[INFO] Downloading checksums.txt" -ForegroundColor Green
    Invoke-WebRequest -Uri "$BaseUrl/checksums.txt" -OutFile $TempChecksums -UseBasicParsing

    $Expected = Get-ExpectedSha -ChecksumsPath $TempChecksums -Filename $ArchiveName
    $Actual = (Get-FileHash -Path $TempZip -Algorithm SHA256).Hash.ToLower()
    if ($Expected -ne $Actual) {
        throw "SHA256 mismatch — refusing to install.`n  expected: $Expected`n  got:      $Actual`nThe download was tampered with or corrupted."
    }
    Write-Host "[INFO] SHA256 verified: $Actual" -ForegroundColor Green

    # Extract
    Write-Host "[INFO] Extracting" -ForegroundColor Green
    if (Test-Path $TempDir) { Remove-Item -Recurse -Force $TempDir }
    Expand-Archive -Path $TempZip -DestinationPath $TempDir
    Copy-Item (Join-Path $TempDir "$BinaryName.exe") -Destination $InstallDir -Force
} finally {
    # Cleanup
    if (Test-Path $TempZip) { Remove-Item $TempZip -Force }
    if (Test-Path $TempChecksums) { Remove-Item $TempChecksums -Force }
    if (Test-Path $TempDir) { Remove-Item -Recurse -Force $TempDir }
}

# Add to PATH if not already there
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
    Write-Host "[INFO] Added $InstallDir to user PATH (restart terminal to apply)" -ForegroundColor Yellow
}

if ($PreviousVersion) {
    Write-Host "[INFO] Upgrade complete: $PreviousVersion -> $Version" -ForegroundColor Green
} else {
    Write-Host "[INFO] Installation complete: $Version" -ForegroundColor Green
}
Write-Host ""
Write-Host "  Next steps:" -ForegroundColor Green
Write-Host "    1. icm init              # configure your AI tools (MCP)"
Write-Host "    2. icm init --mode hook  # install Claude Code hooks"
Write-Host "    3. Restart your AI tool to activate"
Write-Host ""

# icm installer for Windows - https://github.com/rtk-ai/icm
# Usage: irm https://raw.githubusercontent.com/rtk-ai/icm/main/install.ps1 | iex

$ErrorActionPreference = "Stop"

$Repo = "rtk-ai/icm"
$BinaryName = "icm"

function Get-Arch {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch) {
        "X64"   { return "x86_64" }
        "Arm64" { return "aarch64" }
        default { throw "Unsupported architecture: $arch" }
    }
}

function Get-LatestVersion {
    $release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
    return $release.tag_name
}

$Arch = Get-Arch
$Version = Get-LatestVersion
$Target = "$Arch-pc-windows-msvc"
$InstallDir = Join-Path $env:LOCALAPPDATA "icm\bin"

Write-Host "[INFO] Installing $BinaryName $Version ($Arch)..." -ForegroundColor Green

# Create install directory
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null

# Download
$Url = "https://github.com/$Repo/releases/download/$Version/$BinaryName-$Target.zip"
$TempZip = Join-Path $env:TEMP "$BinaryName.zip"
Write-Host "[INFO] Downloading from: $Url" -ForegroundColor Green
Invoke-WebRequest -Uri $Url -OutFile $TempZip

# Extract
$TempDir = Join-Path $env:TEMP "$BinaryName-extract"
if (Test-Path $TempDir) { Remove-Item -Recurse -Force $TempDir }
Expand-Archive -Path $TempZip -DestinationPath $TempDir
Copy-Item (Join-Path $TempDir "$BinaryName.exe") -Destination $InstallDir -Force

# Cleanup
Remove-Item $TempZip -Force
Remove-Item $TempDir -Recurse -Force

# Add to PATH if not already there
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
    Write-Host "[INFO] Added $InstallDir to user PATH (restart terminal to apply)" -ForegroundColor Yellow
}

Write-Host "[INFO] Successfully installed to $InstallDir\$BinaryName.exe" -ForegroundColor Green
Write-Host "[INFO] Run '$BinaryName --help' to get started." -ForegroundColor Green

#Requires -Version 5.1
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\kudzu'),
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'
$BinName = 'kudzu.exe'

function Add-ToUserPath {
    param([string]$Dir)
    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    $parts = @()
    if ($current) { $parts = $current.Split(';') | Where-Object { $_ -ne '' } }
    if ($parts -notcontains $Dir) {
        $new = (($parts + $Dir) -join ';')
        [Environment]::SetEnvironmentVariable('Path', $new, 'User')
        Write-Host "Added $Dir to user PATH (restart your shell to pick it up)."
    }
}

function Remove-FromUserPath {
    param([string]$Dir)
    $current = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $current) { return }
    $parts = $current.Split(';') | Where-Object { $_ -ne '' -and $_ -ne $Dir }
    [Environment]::SetEnvironmentVariable('Path', ($parts -join ';'), 'User')
}

if ($Uninstall) {
    $target = Join-Path $InstallDir $BinName
    if (Test-Path $target) { Remove-Item $target -Force }
    if ((Test-Path $InstallDir) -and -not (Get-ChildItem $InstallDir -Force)) {
        Remove-Item $InstallDir -Force
    }
    Remove-FromUserPath -Dir $InstallDir
    Write-Host "Uninstalled kudzu."
    return
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if (Test-Path (Join-Path $cargoBin 'cargo.exe')) {
        $env:Path = "$cargoBin;$env:Path"
    } else {
        Write-Host "cargo not found. Installing Rust via rustup..."
        $arch = if ([Environment]::Is64BitOperatingSystem) { 'x86_64' } else { 'i686' }
        $url = "https://win.rustup.rs/$arch"
        $installer = Join-Path $env:TEMP 'rustup-init.exe'
        Invoke-WebRequest -Uri $url -OutFile $installer -UseBasicParsing
        & $installer -y --default-toolchain stable --profile minimal
        if ($LASTEXITCODE -ne 0) { throw "rustup-init failed with exit code $LASTEXITCODE" }
        $env:Path = "$cargoBin;$env:Path"
    }
}

Write-Host "Building (cargo build --release)..."
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$source = Join-Path (Get-Location) "target\release\$BinName"
if (-not (Test-Path $source)) { throw "Build output not found: $source" }

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item $source (Join-Path $InstallDir $BinName) -Force
Add-ToUserPath -Dir $InstallDir

Write-Host "Installed $(Join-Path $InstallDir $BinName)"

# Builds the release exe and wraps it in target\installer\hwmon-setup-<version>.exe.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot

$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version\s*=\s*"(.+)"' | Select-Object -First 1).Matches.Groups[1].Value

$iscc = @(
    (Get-Command iscc -ErrorAction SilentlyContinue).Source,
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 not found: winget install JRSoftware.InnoSetup' }

cargo build --release --manifest-path "$root\Cargo.toml"
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$fileVersion = ($version -split '[-+]')[0]
# The icon build.rs drew for this build (Cargo keeps one folder per build script hash).
$icon = Get-ChildItem "$root\target\release\build\hwmon-*\out\hwmon.ico" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $icon) { throw 'hwmon.ico not found under target\release\build; did the build run?' }
& $iscc /Q "/DAppVersion=$version" "/DFileVersion=$fileVersion" "/DIconFile=$($icon.FullName)" "$PSScriptRoot\hwmon.iss"
if ($LASTEXITCODE) { exit $LASTEXITCODE }
Get-Item "$root\target\installer\hwmon-setup-$version.exe"

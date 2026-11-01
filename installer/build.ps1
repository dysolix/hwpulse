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

# Cargo reports where hwmon's build script wrote its files, including the icon it drew;
# older build folders from earlier versions of build.rs may still be lying around.
$messages = cargo build --release --manifest-path "$root\Cargo.toml" --message-format=json-render-diagnostics
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$outDir = $messages | ForEach-Object { $_ | ConvertFrom-Json } |
    Where-Object { $_.reason -eq 'build-script-executed' -and $_.package_id -match '#hwmon@|/hwmon#|\bhwmon ' } |
    Select-Object -Last 1 -ExpandProperty out_dir
$icon = Join-Path "$outDir" 'hwmon.ico'
if (-not $outDir -or -not (Test-Path $icon)) { throw "hwmon.ico not found (build script output: '$outDir')" }
$fileVersion = ($version -split '[-+]')[0]
& $iscc /Q "/DAppVersion=$version" "/DFileVersion=$fileVersion" "/DIconFile=$icon" "$PSScriptRoot\hwmon.iss"
if ($LASTEXITCODE) { exit $LASTEXITCODE }
Get-Item "$root\target\installer\hwmon-setup-$version.exe"

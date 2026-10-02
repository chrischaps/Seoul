# Build a release zip for distribution:
#   dist\seoul-<version>-windows-x64.zip
# containing seoul.exe, presets\, seoul.toml and README.txt.
#   .\packaging\package.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    $version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
    $name = "seoul-$version-windows-x64"

    # A separate target dir, so a running dev build can't lock the exe.
    cargo build --release --target-dir target/package
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

    $stage = Join-Path 'dist' $name
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    New-Item -ItemType Directory -Force $stage | Out-Null

    Copy-Item target/package/release/seoul.exe $stage
    Copy-Item -Recurse presets $stage
    Copy-Item seoul.toml $stage
    Copy-Item packaging/README.txt $stage

    $zip = "dist/$name.zip"
    if (Test-Path $zip) { Remove-Item -Force $zip }
    Compress-Archive -Path $stage -DestinationPath $zip
    $mb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
    Write-Host "Packaged $zip ($mb MB)"
} finally {
    Pop-Location
}

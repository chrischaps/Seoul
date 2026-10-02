# Run Seoul in release mode. Extra arguments pass through, e.g.
#   .\run.ps1 --synth --preset Vortex
#   .\run.ps1 --help
Push-Location $PSScriptRoot
try {
    cargo run --release -- @args
} finally {
    Pop-Location
}
exit $LASTEXITCODE

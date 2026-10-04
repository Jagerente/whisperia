$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$pictures = Join-Path $root 'target\gui-pictures'

$cargo = Get-Command 'cargo' -ErrorAction SilentlyContinue
$cargo = if ($cargo) { $cargo.Source } else { "$env:USERPROFILE\.cargo\bin\cargo.exe" }
if (-not (Test-Path $cargo)) { throw 'cargo not found. Install Rust from https://rustup.rs' }

if (Test-Path $pictures) { Remove-Item -Recurse -Force $pictures -Confirm:$false }

$failed = @()
Push-Location $root
try {
    foreach ($theme in 'dark', 'light') {
        $env:WW_GUI_PICTURES = Join-Path $pictures $theme
        if ($theme -eq 'light') { $env:WW_GUI_LIGHT = '1' } else { Remove-Item Env:WW_GUI_LIGHT -ErrorAction SilentlyContinue }
        & $cargo test -p whisperia-e2e --test gui -- --test-threads=1
        if ($LASTEXITCODE -ne 0) { $failed += $theme }
    }
} finally {
    Remove-Item Env:WW_GUI_PICTURES, Env:WW_GUI_LIGHT -ErrorAction SilentlyContinue
    Pop-Location
}

if ($failed) { throw "some tests failed while taking the $($failed -join ' and ') pictures" }
Write-Host "Ready: $pictures"

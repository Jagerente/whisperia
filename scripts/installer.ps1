param([Parameter(Mandatory)] [string]$Output)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

$cargo = Get-Command 'cargo' -ErrorAction SilentlyContinue
$cargo = if ($cargo) { $cargo.Source } else { "$env:USERPROFILE\.cargo\bin\cargo.exe" }
if (-not (Test-Path $cargo)) { throw 'cargo not found. Install Rust from https://rustup.rs' }

$metadata = & $cargo metadata --no-deps --format-version 1 --manifest-path (Join-Path $root 'Cargo.toml') | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
$version = ($metadata.packages | Where-Object { $_.name -eq 'whisperia' }).version

$installed = @(
    'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1',
    'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1',
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\Inno Setup 6_is1'
) | Where-Object { Test-Path $_ } | ForEach-Object { Join-Path (Get-ItemProperty $_).InstallLocation 'ISCC.exe' } |
    Where-Object { Test-Path $_ } | Select-Object -First 1
$iscc = if ($installed) { $installed } else { (Get-Command 'ISCC.exe' -ErrorAction SilentlyContinue).Source }
if (-not $iscc) { throw 'ISCC.exe not found. Install Inno Setup 6 from https://jrsoftware.org/isinfo.php' }

& $iscc /Q "/DVersion=$version" "/O$Output" (Join-Path $PSScriptRoot 'installer.iss')
if ($LASTEXITCODE -ne 0) { throw 'ISCC failed' }
Write-Host "Ready: $(Join-Path $Output "whisperia-client-$version-windows-x64-setup.exe")"

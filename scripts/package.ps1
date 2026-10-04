$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$dist = Join-Path $root 'dist'

$cargo = Get-Command 'cargo' -ErrorAction SilentlyContinue
$cargo = if ($cargo) { $cargo.Source } else { "$env:USERPROFILE\.cargo\bin\cargo.exe" }
if (-not (Test-Path $cargo)) { throw 'cargo not found. Install Rust from https://rustup.rs' }

Push-Location $root
try {
    & $cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
} finally {
    Pop-Location
}

$redist = Get-ChildItem 'C:\Program Files*\Microsoft Visual Studio\*\*\VC\Redist\MSVC\*\x64\Microsoft.VC*.CRT\vcruntime140.dll' -ErrorAction SilentlyContinue |
    Sort-Object { $_.VersionInfo.FileVersionRaw } -Descending | Select-Object -First 1 | ForEach-Object { $_.DirectoryName }
if (-not $redist) { $redist = "$env:WINDIR\System32" }
function Find-Runtime([string[]]$names) {
    foreach ($name in $names) {
        $dll = Join-Path $redist $name
        if (Test-Path $dll) { $dll } else { Write-Warning "$name not found; target PCs will need the Visual C++ redistributable." }
    }
}

function Install-File([string]$source, [string]$folder) {
    $name = Split-Path -Leaf $source
    $target = Join-Path $folder $name
    try {
        Copy-Item $source $target -Force
    } catch [System.IO.IOException] {
        Rename-Item $target "$name.old-$([DateTime]::Now.Ticks)"
        Copy-Item $source $target
        Write-Host "$name is in use: the running one goes on, the next start takes the new one."
    }
}

$packages = @(
    @{ Role = 'client'; Exes = @('whisperia.exe')
       Runtime = @('vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll') },
    @{ Role = 'server'; Exes = @('whisperia-server.exe')
       Runtime = @('vcruntime140.dll') }
)
foreach ($package in $packages) {
    $folder = Join-Path $dist "whisperia-$($package.Role)"
    New-Item -ItemType Directory -Force $folder | Out-Null
    Get-ChildItem $folder -Filter '*.old-*' | Remove-Item -Force -Confirm:$false -ErrorAction SilentlyContinue

    $sources = @($package.Exes | ForEach-Object { Join-Path $root "target\release\$_" })
    $sources += @(Find-Runtime $package.Runtime)
    $sources += @(Join-Path $root 'LICENSE')
    $sources | ForEach-Object { Install-File $_ $folder }
}

Get-ChildItem $dist -Directory | Where-Object { $_.Name -in 'whisperia-client', 'whisperia-server' } |
    Select-Object Name, @{ n = 'MB'; e = { [math]::Round((Get-ChildItem $_.FullName -File | Measure-Object Length -Sum).Sum / 1MB, 1) } } |
    Format-Table -AutoSize
Write-Host "Ready: $dist"

# Installs tinstar from the GitHub releases (Windows, x86_64).
#
#   irm https://raw.githubusercontent.com/matpdev/tinstar/main/install.ps1 | iex
#
# $env:TINSTAR_VERSION      a release tag such as v0.1.0 (default: the latest release)
# $env:TINSTAR_INSTALL_DIR  where the binary goes (default: %LOCALAPPDATA%\tinstar\bin)
$ErrorActionPreference = 'Stop'

$repo = 'matpdev/tinstar'
$version = if ($env:TINSTAR_VERSION) { $env:TINSTAR_VERSION } else { 'latest' }
$dir = if ($env:TINSTAR_INSTALL_DIR) { $env:TINSTAR_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'tinstar\bin' }
$name = 'tinstar-x86_64-pc-windows-msvc'
$asset = "$name.zip"
$url = if ($version -eq 'latest') {
    "https://github.com/$repo/releases/latest/download/$asset"
} else {
    "https://github.com/$repo/releases/download/$version/$asset"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
New-Item -ItemType Directory $tmp | Out-Null
try {
    Write-Host "Downloading $url"
    Invoke-WebRequest $url -OutFile (Join-Path $tmp $asset)
    Invoke-WebRequest "$url.sha256" -OutFile (Join-Path $tmp "$asset.sha256")

    $expected = ((Get-Content (Join-Path $tmp "$asset.sha256")) -split '\s+')[0]
    $actual = (Get-FileHash (Join-Path $tmp $asset) -Algorithm SHA256).Hash
    if ($expected -ne $actual.ToLower()) { throw "checksum mismatch for $asset" }

    Expand-Archive (Join-Path $tmp $asset) -DestinationPath $tmp
    New-Item -ItemType Directory -Force $dir | Out-Null
    Copy-Item (Join-Path $tmp "$name\tinstar.exe") (Join-Path $dir 'tinstar.exe') -Force
} finally {
    Remove-Item -Recurse -Force $tmp
}

$exe = Join-Path $dir 'tinstar.exe'
Write-Host "Installed $(& $exe --version) to $exe"
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $dir) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User')
    Write-Host "Added $dir to your PATH (open a new terminal to use it)."
}

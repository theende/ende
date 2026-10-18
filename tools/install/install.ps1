#!/usr/bin/env pwsh
# Copyright 2026 the Ende authors. MIT license.
#
# Installs Ende on Windows. Served at https://ende.sh/install.ps1.
#
#   irm https://ende.sh/install.ps1 | iex
#   $v = "v2026.10.17"; irm https://ende.sh/install.ps1 | iex
#
# The binary goes in $env:ENDE_INSTALL_ROOT\bin (default ~\.ende\bin), the
# same directory `ende install -g` puts its shims in, and that directory is
# added to the user PATH.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# Windows PowerShell 5.1 does not enable TLS 1.2 by default.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

if (-not [Environment]::Is64BitOperatingSystem) {
  Write-Error 'Ende has no release for 32-bit Windows.'
}
# Ende only ships an x64 build; Windows on ARM runs it under emulation.
$Target = 'x86_64-pc-windows-msvc'

$Version = if ($v) {
  $v
} elseif ($args.Length -eq 1) {
  $args.Get(0)
} else {
  (Invoke-RestMethod -UseBasicParsing 'https://dl.ende.sh/release-latest.txt').Trim()
}
if (-not $Version.StartsWith('v')) {
  $Version = "v$Version"
}

$Root = if ($env:ENDE_INSTALL_ROOT) { $env:ENDE_INSTALL_ROOT } else { "$HOME\.ende" }
$BinDir = "$Root\bin"
$Zip = "ende-$Target.zip"
$Url = "https://github.com/theende/ende/releases/download/$Version/$Zip"

$Tmp = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $Tmp | Out-Null
try {
  Write-Output "Downloading Ende $Version for $Target..."
  Invoke-WebRequest -UseBasicParsing -OutFile "$Tmp\$Zip" $Url
  Invoke-WebRequest -UseBasicParsing -OutFile "$Tmp\$Zip.sha256sum" "$Url.sha256sum"

  $Expected = ((Get-Content "$Tmp\$Zip.sha256sum" -Raw) -split '\s+')[0]
  $Actual = (Get-FileHash -Algorithm SHA256 "$Tmp\$Zip").Hash
  if ($Actual -ne $Expected) {
    Write-Error "Checksum mismatch for $Zip (expected $Expected, got $Actual)"
  }

  Expand-Archive -Force -Path "$Tmp\$Zip" -DestinationPath $Tmp
  New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
  Move-Item -Force "$Tmp\ende.exe" "$BinDir\ende.exe"
} finally {
  Remove-Item -Recurse -Force $Tmp
}

$UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not $UserPath) {
  [Environment]::SetEnvironmentVariable('Path', $BinDir, 'User')
  $env:Path = "$BinDir;$env:Path"
} elseif (($UserPath -split ';') -notcontains $BinDir) {
  [Environment]::SetEnvironmentVariable('Path', "$BinDir;$UserPath", 'User')
  $env:Path = "$BinDir;$env:Path"
}

Write-Output "Ende was installed to $BinDir\ende.exe"
& "$BinDir\ende.exe" --version
Write-Output "Run 'ende --help' to get started (restart your terminal first)."

param(
  [Parameter(Mandatory = $true)]
  [ValidateSet('production', 'developer')]
  [string]$Channel
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$ProjectRoot = Split-Path -Parent $Root
$Target = Join-Path $Root 'src-tauri\target\release'
$Dist = Join-Path $Root "dist\$Channel"
$Package = Get-Content (Join-Path $Root 'package.json') -Raw | ConvertFrom-Json
$Version = [string]$Package.version
if ([string]::IsNullOrWhiteSpace($Version)) {
  throw 'package.json 缺少版本号'
}
Remove-Item $Dist -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

if ($Channel -eq 'production') {
  & node (Join-Path $Root 'scripts\verify-build-boundary.mjs')
  if ($LASTEXITCODE -ne 0) { throw '生产构建边界校验失败' }
  Copy-Item (Join-Path $Target 'dh-bot.exe') (Join-Path $Dist 'DH-BOT.exe')
  $Installer = Get-ChildItem (Join-Path $Target 'bundle\nsis') -Filter '*.exe' | Select-Object -First 1
  if (-not $Installer) { throw '没有找到 Tauri NSIS 安装器' }
  $InstallerTarget = Join-Path $Dist "DH-BOT-$Version-windows-x64-setup.exe"
  Copy-Item $Installer.FullName $InstallerTarget
  Copy-Item (Join-Path $ProjectRoot 'docs\DH使用手册.md') (Join-Path $Dist 'DH-Manual-ZH.md')
  Copy-Item (Join-Path $ProjectRoot 'docs\DH-Manual-ZH.pdf') (Join-Path $Dist 'DH-Manual-ZH.pdf')
  Copy-Item (Join-Path $ProjectRoot 'package\DH-BOT-Default-Rules.json') (Join-Path $Dist 'DH-BOT-Default-Rules.json')
  & (Join-Path $Root 'scripts\verify-windows-production.ps1') -ArtifactDirectory $Dist
  if ($LASTEXITCODE -ne 0) { throw 'Windows 生产产物深度扫描失败' }
  $Zip = Join-Path $Root "dist\DH-BOT-$Version-windows-x64-portable.zip"
  $PortableStage = Join-Path $Root ("dist\portable-" + [guid]::NewGuid().ToString('N'))
  Remove-Item $Zip -Force -ErrorAction SilentlyContinue
  try {
    New-Item -ItemType Directory -Force -Path $PortableStage | Out-Null
    Copy-Item (Join-Path $Dist 'DH-BOT.exe') (Join-Path $PortableStage 'DH-BOT-Portable.exe')
    Copy-Item (Join-Path $Dist 'DH-Manual-ZH.md') $PortableStage
    Copy-Item (Join-Path $Dist 'DH-Manual-ZH.pdf') $PortableStage
    Copy-Item (Join-Path $Dist 'DH-BOT-Default-Rules.json') $PortableStage
    & node (Join-Path $Root 'scripts\verify-production.mjs') $PortableStage
    if ($LASTEXITCODE -ne 0) { throw '便携包生产隔离校验失败' }
    Compress-Archive -Path (Join-Path $PortableStage '*') -DestinationPath $Zip
    $VerifyZip = Join-Path ([System.IO.Path]::GetTempPath()) ("dh-bot-production-" + [guid]::NewGuid().ToString('N'))
    Expand-Archive -Path $Zip -DestinationPath $VerifyZip
    & node (Join-Path $Root 'scripts\verify-production.mjs') $VerifyZip
    if ($LASTEXITCODE -ne 0) { throw '便携包生产隔离校验失败' }
  } finally {
    if ($VerifyZip) { Remove-Item $VerifyZip -Recurse -Force -ErrorAction SilentlyContinue }
    Remove-Item $PortableStage -Recurse -Force -ErrorAction SilentlyContinue
  }
} else {
  Copy-Item (Join-Path $Target 'dh-bot.exe') (Join-Path $Dist 'DH-BOT-Dev.exe')
  Copy-Item (Join-Path $Target 'dh-fixture.exe') (Join-Path $Dist 'DH-Fixture.exe')
  Copy-Item (Join-Path $Root 'README-Developer.md') $Dist
  $Zip = Join-Path $Root "dist\DH-BOT-$Version-dev-windows-x64.zip"
  Remove-Item $Zip -Force -ErrorAction SilentlyContinue
  Compress-Archive -Path (Join-Path $Dist '*') -DestinationPath $Zip
}

$HashFiles = @(Get-ChildItem $Dist -File | Where-Object { $_.Name -ne 'SHA256SUMS.txt' })
if (Test-Path $Zip) { $HashFiles += Get-Item $Zip }
$Hashes = $HashFiles | Sort-Object Name | ForEach-Object {
  $Hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
  "$Hash  $($_.Name)"
}
$Hashes | Set-Content (Join-Path $Dist 'SHA256SUMS.txt') -Encoding ascii
Write-Host "$Channel Windows 产物已整理：$Dist"

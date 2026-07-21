param(
  [ValidateSet('production', 'developer')]
  [string]$Channel
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Target = Join-Path $Root 'src-tauri\target\release'
$Dist = Join-Path $Root "dist\$Channel"
$Version = '3.0.0-beta.1'

function Sign-Artifact([string]$Path) {
  if (-not $env:DH_SIGN_PFX) { return }
  $Arguments = @('sign', '/fd', 'SHA256', '/tr', 'http://timestamp.digicert.com', '/td', 'SHA256', '/f', $env:DH_SIGN_PFX)
  if ($env:DH_SIGN_PASSWORD) { $Arguments += @('/p', $env:DH_SIGN_PASSWORD) }
  $Arguments += $Path
  & signtool @Arguments
  if ($LASTEXITCODE -ne 0) { throw "Authenticode 签名失败：$Path" }
  & signtool verify /pa /all $Path
  if ($LASTEXITCODE -ne 0) { throw "Authenticode 校验失败：$Path" }
}

Remove-Item $Dist -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

if ($Channel -eq 'production') {
  Copy-Item (Join-Path $Target 'dh-bot.exe') (Join-Path $Dist 'DH-BOT.exe')
  Get-ChildItem (Join-Path $Target 'bundle\nsis') -Filter '*.exe' | Copy-Item -Destination $Dist
  Get-ChildItem $Dist -Filter '*.exe' | ForEach-Object { Sign-Artifact $_.FullName }
  & node (Join-Path $Root 'scripts\verify-production.mjs') $Dist
  $Zip = Join-Path $Root "dist\DH-BOT-$Version-windows-x64-portable.zip"
  Remove-Item $Zip -Force -ErrorAction SilentlyContinue
  Compress-Archive -Path (Join-Path $Dist 'DH-BOT.exe') -DestinationPath $Zip
} else {
  Copy-Item (Join-Path $Target 'dh-bot.exe') (Join-Path $Dist 'DH-BOT-Dev.exe')
  Copy-Item (Join-Path $Target 'dh-fixture.exe') (Join-Path $Dist 'DH-Fixture.exe')
  Get-ChildItem $Dist -Filter '*.exe' | ForEach-Object { Sign-Artifact $_.FullName }
  Copy-Item (Join-Path $Root 'README-Developer.md') $Dist
  $Zip = Join-Path $Root "dist\DH-BOT-$Version-dev-windows-x64.zip"
  Remove-Item $Zip -Force -ErrorAction SilentlyContinue
  Compress-Archive -Path (Join-Path $Dist '*') -DestinationPath $Zip
}

$Hashes = Get-ChildItem $Dist -File | Sort-Object Name | ForEach-Object {
  $Hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
  "$Hash  $($_.Name)"
}
$Hashes | Set-Content (Join-Path $Dist 'SHA256SUMS.txt') -Encoding ascii
Write-Host "$Channel Windows 产物已整理：$Dist"

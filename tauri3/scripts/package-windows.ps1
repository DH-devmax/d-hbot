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
$RequireSignature = ($env:DH_RELEASE_REQUIRE_SIGNATURE -eq '1') -or ($env:GITHUB_REF -like 'refs/tags/v3.*')
$PreSigned = $env:DH_RELEASE_PRE_SIGNED -eq '1'

if ($RequireSignature) {
  $ReleaseTag = $env:DH_RELEASE_TAG
  if ([string]::IsNullOrWhiteSpace($ReleaseTag) -and $env:GITHUB_REF -like 'refs/tags/v3.*') {
    $ReleaseTag = $env:GITHUB_REF.Substring('refs/tags/'.Length)
  }
  if ($ReleaseTag -notmatch '^v3\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z][0-9A-Za-z.-]*)?$') {
    throw "正式发布标签格式不正确：$ReleaseTag"
  }
  $TagVersion = $ReleaseTag.Substring(1)
  if ($TagVersion -ne $Version) {
    throw "正式标签版本 $TagVersion 与应用版本 $Version 不一致"
  }
}

if ($RequireSignature -and -not $env:DH_SIGN_PFX -and -not $PreSigned) {
  throw '正式标签发布必须配置 Authenticode 证书 DH_SIGN_PFX'
}

function Sign-Artifact([string]$Path) {
  if (-not $env:DH_SIGN_PFX) { return }
  $ExistingSignature = Get-AuthenticodeSignature -FilePath $Path
  if ($ExistingSignature.Status -eq 'Valid') {
    & signtool verify /pa /all $Path
    if ($LASTEXITCODE -ne 0) { throw "现有 Authenticode 签名校验失败：$Path" }
    return
  }
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
  & node (Join-Path $Root 'scripts\verify-build-boundary.mjs')
  if ($LASTEXITCODE -ne 0) { throw '生产构建边界校验失败' }
  if ($RequireSignature) {
    & signtool verify /pa /all (Join-Path $Target 'dh-bot.exe')
    if ($LASTEXITCODE -ne 0) { throw 'NSIS 打包前主程序未通过 Authenticode 校验' }
  }
  Copy-Item (Join-Path $Target 'dh-bot.exe') (Join-Path $Dist 'DH-BOT.exe')
  $Installer = Get-ChildItem (Join-Path $Target 'bundle\nsis') -Filter '*.exe' | Select-Object -First 1
  if (-not $Installer) { throw '没有找到 Tauri NSIS 安装器' }
  $InstallerTarget = Join-Path $Dist "DH-BOT-$Version-windows-x64-setup.exe"
  Copy-Item $Installer.FullName $InstallerTarget
  Copy-Item (Join-Path $ProjectRoot 'docs\DH使用手册.md') (Join-Path $Dist 'DH-Manual-ZH.md')
  Copy-Item (Join-Path $ProjectRoot 'docs\DH-Manual-ZH.pdf') (Join-Path $Dist 'DH-Manual-ZH.pdf')
  Copy-Item (Join-Path $ProjectRoot 'package\DH-BOT-Default-Rules.json') (Join-Path $Dist 'DH-BOT-Default-Rules.json')
  Get-ChildItem $Dist -Filter '*.exe' | ForEach-Object { Sign-Artifact $_.FullName }
  & (Join-Path $Root 'scripts\verify-windows-production.ps1') -ArtifactDirectory $Dist -RequireSignature:$RequireSignature
  if ($LASTEXITCODE -ne 0) { throw 'Windows 生产产物深度扫描失败' }
  $Zip = Join-Path $Root "dist\DH-BOT-$Version-windows-x64-portable.zip"
  Remove-Item $Zip -Force -ErrorAction SilentlyContinue
  $PortableFiles = @(
    (Join-Path $Dist 'DH-BOT.exe'),
    (Join-Path $Dist 'DH-Manual-ZH.md'),
    (Join-Path $Dist 'DH-Manual-ZH.pdf'),
    (Join-Path $Dist 'DH-BOT-Default-Rules.json')
  )
  Compress-Archive -Path $PortableFiles -DestinationPath $Zip
  $VerifyZip = Join-Path ([System.IO.Path]::GetTempPath()) ("dh-bot-production-" + [guid]::NewGuid().ToString('N'))
  try {
    Expand-Archive -Path $Zip -DestinationPath $VerifyZip
    & node (Join-Path $Root 'scripts\verify-production.mjs') $VerifyZip
    if ($LASTEXITCODE -ne 0) { throw '便携包生产隔离校验失败' }
  } finally {
    Remove-Item $VerifyZip -Recurse -Force -ErrorAction SilentlyContinue
  }
} else {
  Copy-Item (Join-Path $Target 'dh-bot.exe') (Join-Path $Dist 'DH-BOT-Dev.exe')
  Copy-Item (Join-Path $Target 'dh-fixture.exe') (Join-Path $Dist 'DH-Fixture.exe')
  Get-ChildItem $Dist -Filter '*.exe' | ForEach-Object { Sign-Artifact $_.FullName }
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

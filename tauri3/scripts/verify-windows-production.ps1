param(
  [Parameter(Mandatory = $true)]
  [string]$ArtifactDirectory
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Artifacts = (Resolve-Path $ArtifactDirectory).Path

& node (Join-Path $Root 'scripts\verify-production.mjs') $Artifacts
if ($LASTEXITCODE -ne 0) { throw '生产产物直接扫描失败' }

$MainExecutable = Join-Path $Artifacts 'DH-BOT.exe'
if (-not (Test-Path $MainExecutable)) { throw '生产产物缺少 DH-BOT.exe' }
$Signature = Get-AuthenticodeSignature -FilePath $MainExecutable
Write-Host "DH-BOT.exe 签名状态：$($Signature.Status)（个人云盘分发允许未签名）"

$SevenZip = Get-Command 7z.exe -ErrorAction SilentlyContinue
if (-not $SevenZip) { $SevenZip = Get-Command 7z -ErrorAction SilentlyContinue }
if (-not $SevenZip) { throw '深度扫描 NSIS 需要发布机安装 7-Zip' }

$Installers = Get-ChildItem $Artifacts -File -Filter '*.exe' |
  Where-Object { $_.Name -ne 'DH-BOT.exe' }
if (@($Installers).Count -eq 0) { throw '生产产物中没有 NSIS 安装器' }

foreach ($Installer in $Installers) {
  $Expanded = Join-Path ([System.IO.Path]::GetTempPath()) ("dh-bot-nsis-" + [guid]::NewGuid().ToString('N'))
  try {
    New-Item -ItemType Directory -Force -Path $Expanded | Out-Null
    & $SevenZip.Source x -y "-o$Expanded" $Installer.FullName | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "NSIS 解包失败：$($Installer.Name)" }
    & node (Join-Path $Root 'scripts\verify-production.mjs') $Expanded --allow-no-executable
    if ($LASTEXITCODE -ne 0) { throw "NSIS 内容生产隔离校验失败：$($Installer.Name)" }
  } finally {
    Remove-Item $Expanded -Recurse -Force -ErrorAction SilentlyContinue
  }
}

Write-Host "Windows 生产产物深度扫描通过：$(@($Installers).Count) 个 NSIS 安装器"

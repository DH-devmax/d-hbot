param(
  [Parameter(Mandatory = $true)]
  [string]$ArtifactDirectory,
  [switch]$RequireSignature
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Artifacts = (Resolve-Path $ArtifactDirectory).Path

& node (Join-Path $Root 'scripts\verify-production.mjs') $Artifacts
if ($LASTEXITCODE -ne 0) { throw '生产产物直接扫描失败' }

function Assert-Authenticode([string]$Path) {
  if (-not $RequireSignature) { return }
  & signtool verify /pa /all $Path
  if ($LASTEXITCODE -ne 0) { throw "Authenticode 校验失败：$Path" }
}

if ($RequireSignature) {
  $MainExecutable = Join-Path $Artifacts 'DH-BOT.exe'
  if (-not (Test-Path $MainExecutable)) { throw '生产产物缺少 DH-BOT.exe' }
  Assert-Authenticode $MainExecutable
}

$SevenZip = Get-Command 7z.exe -ErrorAction SilentlyContinue
if (-not $SevenZip) { $SevenZip = Get-Command 7z -ErrorAction SilentlyContinue }
if (-not $SevenZip) { throw '深度扫描 NSIS 需要发布机安装 7-Zip' }

$Installers = Get-ChildItem $Artifacts -File -Filter '*.exe' |
  Where-Object { $_.Name -ne 'DH-BOT.exe' }
if (@($Installers).Count -eq 0) { throw '生产产物中没有 NSIS 安装器' }

foreach ($Installer in $Installers) {
  Assert-Authenticode $Installer.FullName
  $Expanded = Join-Path ([System.IO.Path]::GetTempPath()) ("dh-bot-nsis-" + [guid]::NewGuid().ToString('N'))
  try {
    New-Item -ItemType Directory -Force -Path $Expanded | Out-Null
    & $SevenZip.Source x -y "-o$Expanded" $Installer.FullName | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "NSIS 解包失败：$($Installer.Name)" }
    & node (Join-Path $Root 'scripts\verify-production.mjs') $Expanded --allow-no-executable
    if ($LASTEXITCODE -ne 0) { throw "NSIS 内容生产隔离校验失败：$($Installer.Name)" }
    if ($RequireSignature) {
      $EmbeddedExecutables = @(Get-ChildItem $Expanded -Recurse -File -Filter '*.exe' |
        Where-Object { $_.Name -match '^dh-bot.*\.exe$' })
      if ($EmbeddedExecutables.Count -eq 0) {
        throw "NSIS 内没有找到嵌入的 DH BOT 主程序：$($Installer.Name)"
      }
      $EmbeddedExecutables | ForEach-Object { Assert-Authenticode $_.FullName }
    }
  } finally {
    Remove-Item $Expanded -Recurse -Force -ErrorAction SilentlyContinue
  }
}

Write-Host "Windows 生产产物深度扫描通过：$(@($Installers).Count) 个 NSIS 安装器"

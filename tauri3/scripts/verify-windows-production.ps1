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
$SevenZipPath = if ($SevenZip) { $SevenZip.Source } else { '' }
if (-not $SevenZipPath) {
  $SevenZipPath = @(
    (Join-Path $env:ProgramFiles '7-Zip\7z.exe'),
    $(if (${env:ProgramFiles(x86)}) { Join-Path ${env:ProgramFiles(x86)} '7-Zip\7z.exe' })
  ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
}
if (-not $SevenZipPath) { throw '深度扫描 NSIS 需要发布机安装 7-Zip' }

$Installers = Get-ChildItem $Artifacts -File -Filter '*.exe' |
  Where-Object { $_.Name -notin @('DH-BOT.exe', 'DH-BOT-Portable.exe') }
if (@($Installers).Count -eq 0) { throw '生产产物中没有 NSIS 安装器' }

$PortableArchives = @(Get-ChildItem $Artifacts -File -Filter '*-portable.zip')
if ($PortableArchives.Count -ne 1) {
  throw "生产产物必须包含且只包含一个 portable ZIP，当前数量：$($PortableArchives.Count)"
}

foreach ($Installer in $Installers) {
  $Expanded = Join-Path ([System.IO.Path]::GetTempPath()) ("dh-bot-nsis-" + [guid]::NewGuid().ToString('N'))
  try {
    New-Item -ItemType Directory -Force -Path $Expanded | Out-Null
    & $SevenZipPath x -y "-o$Expanded" $Installer.FullName | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "NSIS 解包失败：$($Installer.Name)" }
    & node (Join-Path $Root 'scripts\verify-production.mjs') $Expanded --allow-no-executable
    if ($LASTEXITCODE -ne 0) { throw "NSIS 内容生产隔离校验失败：$($Installer.Name)" }
  } finally {
    Remove-Item $Expanded -Recurse -Force -ErrorAction SilentlyContinue
  }
}

foreach ($Archive in $PortableArchives) {
  $Expanded = Join-Path ([System.IO.Path]::GetTempPath()) ("dh-bot-portable-" + [guid]::NewGuid().ToString('N'))
  try {
    Expand-Archive -LiteralPath $Archive.FullName -DestinationPath $Expanded
    $PortableExecutable = Join-Path $Expanded 'DH-BOT-Portable.exe'
    if (-not (Test-Path -LiteralPath $PortableExecutable -PathType Leaf)) {
      throw "portable ZIP 缺少 DH-BOT-Portable.exe：$($Archive.Name)"
    }
    & node (Join-Path $Root 'scripts\verify-production.mjs') $Expanded
    if ($LASTEXITCODE -ne 0) { throw "portable ZIP 生产隔离校验失败：$($Archive.Name)" }
  } finally {
    Remove-Item $Expanded -Recurse -Force -ErrorAction SilentlyContinue
  }
}

$ManifestPath = Join-Path $Artifacts 'SHA256SUMS.txt'
if (-not (Test-Path -LiteralPath $ManifestPath -PathType Leaf)) {
  throw '生产产物缺少 SHA256SUMS.txt'
}
$ExpectedNames = @{}
foreach ($Line in Get-Content -LiteralPath $ManifestPath) {
  if ($Line -notmatch '^([0-9a-fA-F]{64})  (.+)$') {
    throw "SHA256SUMS.txt 格式无效：$Line"
  }
  $ExpectedHash = $Matches[1].ToLowerInvariant()
  $Name = $Matches[2]
  if ($Name -ne [System.IO.Path]::GetFileName($Name)) {
    throw "SHA256SUMS.txt 包含非文件名路径：$Name"
  }
  if ($ExpectedNames.ContainsKey($Name)) {
    throw "SHA256SUMS.txt 包含重复文件：$Name"
  }
  $Path = Join-Path $Artifacts $Name
  if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    throw "SHA256SUMS.txt 引用缺失文件：$Name"
  }
  $ActualHash = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($ActualHash -ne $ExpectedHash) {
    throw "SHA256 校验失败：$Name"
  }
  $ExpectedNames[$Name] = $true
}

$Unlisted = @(Get-ChildItem $Artifacts -File |
  Where-Object { $_.Name -ne 'SHA256SUMS.txt' -and -not $ExpectedNames.ContainsKey($_.Name) })
if ($Unlisted.Count -gt 0) {
  throw "SHA256SUMS.txt 未列出文件：$(($Unlisted.Name | Sort-Object) -join ', ')"
}

Write-Host "Windows 生产产物深度扫描通过：$(@($Installers).Count) 个 NSIS 安装器，$($PortableArchives.Count) 个 portable ZIP，$($ExpectedNames.Count) 项哈希"

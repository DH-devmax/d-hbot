$ErrorActionPreference = "Stop"

Push-Location $PSScriptRoot
try {
    go test ./...
    New-Item -ItemType Directory -Force -Path build | Out-Null
    New-Item -ItemType Directory -Force -Path build/tools/diagnostic | Out-Null
    go build -trimpath -ldflags "-s -w" -o build/tools/diagnostic/DHBridge.exe ./cmd/dh-bridge
    $resourceDir = Join-Path $env:TEMP ("dh-bot-rsrc-" + [Guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Force -Path $resourceDir | Out-Null
    $resourcePath = Join-Path $resourceDir "rsrc_windows_amd64.syso"
    go run github.com/akavel/rsrc@v0.10.2 -arch amd64 -ico assets/icon.ico,assets/tray.ico -manifest cmd/dh/dh.exe.manifest -o $resourcePath
    Move-Item -Force $resourcePath cmd/dh/rsrc_windows_amd64.syso
    Remove-Item -Recurse -Force $resourceDir
    go build -trimpath -ldflags "-s -w -H=windowsgui" -o build/DH-BOT.exe ./cmd/dh
    if ($env:SIGN_CERT_SHA1) {
        $timestamp = if ($env:SIGN_TIMESTAMP_URL) { $env:SIGN_TIMESTAMP_URL } else { "http://timestamp.digicert.com" }
        & signtool.exe sign /sha1 $env:SIGN_CERT_SHA1 /fd SHA256 /tr $timestamp /td SHA256 build/DH-BOT.exe
    }
    Write-Host "Built DH BOT 2.7.0: $PSScriptRoot\build\DH-BOT.exe"
    Write-Host "Diagnostic bridge: $PSScriptRoot\build\tools\diagnostic\DHBridge.exe"
} finally {
    Pop-Location
}

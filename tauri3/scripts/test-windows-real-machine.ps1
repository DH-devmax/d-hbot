param(
  [Parameter(Mandatory = $true)]
  [string]$Artifact,
  [string]$ExpectedSha256 = '',
  [string]$OutputDirectory = '',
  [ValidateRange(10, 120)]
  [int]$StartupTimeoutSeconds = 45,
  [switch]$SkipLaunch,
  [switch]$VerifyInteractiveExit
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

function Resolve-FullPath([string]$Path) {
  return [System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Path).Path)
}

function Add-Result(
  [string]$Name,
  [ValidateSet('passed', 'failed', 'warning', 'info')]
  [string]$Status,
  [string]$Detail
) {
  $script:Results.Add([ordered]@{
    name = $Name
    status = $Status
    detail = $Detail
    checkedAt = [DateTime]::UtcNow.ToString('o')
  })
  $Color = switch ($Status) {
    'passed' { 'Green' }
    'failed' { 'Red' }
    'warning' { 'Yellow' }
    default { 'Gray' }
  }
  Write-Host ("[{0}] {1}: {2}" -f $Status.ToUpperInvariant(), $Name, $Detail) -ForegroundColor $Color
}

function Get-HttpProbe([string]$Url) {
  try {
    Add-Type -AssemblyName System.Net.Http
    $Handler = [System.Net.Http.HttpClientHandler]::new()
    $Handler.UseProxy = $false
    $Client = [System.Net.Http.HttpClient]::new($Handler)
    $Client.Timeout = [TimeSpan]::FromSeconds(5)
    try {
      $Response = $Client.GetAsync($Url).GetAwaiter().GetResult()
      $Body = $Response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
      return [ordered]@{
        url = $Url
        success = $Response.IsSuccessStatusCode
        statusCode = [int]$Response.StatusCode
        bodyPreview = if ($Body.Length -gt 240) { $Body.Substring(0, 240) } else { $Body }
        error = ''
      }
    } finally {
      $Client.Dispose()
      $Handler.Dispose()
    }
  } catch {
    return [ordered]@{
      url = $Url
      success = $false
      statusCode = 0
      bodyPreview = ''
      error = $_.Exception.Message
    }
  }
}

function Get-ProcessSnapshot {
  return @(Get-CimInstance Win32_Process | ForEach-Object {
    [ordered]@{
      pid = [int]$_.ProcessId
      parentPid = [int]$_.ParentProcessId
      name = [string]$_.Name
      path = [string]$_.ExecutablePath
      commandLine = [string]$_.CommandLine
      creationDate = if ($_.CreationDate) { ([DateTime]$_.CreationDate).ToUniversalTime().ToString('o') } else { '' }
    }
  })
}

function Get-DescendantIds([int]$RootPid, [object[]]$Snapshot) {
  $Known = [System.Collections.Generic.HashSet[int]]::new()
  [void]$Known.Add($RootPid)
  do {
    $Changed = $false
    foreach ($Item in $Snapshot) {
      if ($Known.Contains([int]$Item.parentPid) -and -not $Known.Contains([int]$Item.pid)) {
        [void]$Known.Add([int]$Item.pid)
        $Changed = $true
      }
    }
  } while ($Changed)
  return @($Known | Where-Object { $_ -ne $RootPid })
}

function Get-WindowSnapshot([int]$ProcessId, [long]$KnownHandle = 0) {
  $Process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
  if (-not $Process) { return $null }
  $Handle = if ($KnownHandle) {
    [IntPtr]$KnownHandle
  } else {
    [DhBotRealMachine.NativeWindow]::FindMainWindow($ProcessId)
  }
  if ($Handle -eq [IntPtr]::Zero) {
    return [ordered]@{ handle = 0; visible = $false; minimized = $false; title = '' }
  }
  return [ordered]@{
    handle = $Handle.ToInt64()
    visible = [DhBotRealMachine.NativeWindow]::IsWindowVisible($Handle)
    minimized = [DhBotRealMachine.NativeWindow]::IsIconic($Handle)
    title = [DhBotRealMachine.NativeWindow]::GetWindowTitle($Handle)
  }
}

function Write-Reports([string]$Directory, [System.Collections.IDictionary]$Report) {
  New-Item -ItemType Directory -Force -Path $Directory | Out-Null
  $JsonPath = Join-Path $Directory 'DH-BOT-Windows-Real-Machine.json'
  $MarkdownPath = Join-Path $Directory 'DH-BOT-Windows-Real-Machine.md'
  $Report | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $JsonPath -Encoding utf8

  $Lines = [System.Collections.Generic.List[string]]::new()
  $Lines.Add('# DH BOT Windows 实机验收报告')
  $Lines.Add('')
  $Lines.Add("- 生成时间：$($Report.generatedAt)")
  $Lines.Add("- Windows：$($Report.environment.osVersion)")
  $Lines.Add("- 产物：$($Report.artifact.path)")
  $Lines.Add("- SHA-256：``$($Report.artifact.sha256)``")
  $Lines.Add('')
  $Lines.Add('| 检查项 | 结果 | 详情 |')
  $Lines.Add('| --- | --- | --- |')
  foreach ($Result in $Report.results) {
    $Detail = ([string]$Result.detail).Replace('|', '\|').Replace("`r", ' ').Replace("`n", ' ')
    $Lines.Add("| $($Result.name) | $($Result.status) | $Detail |")
  }
  $Lines.Add('')
  $Lines.Add('## 仍需人工确认')
  $Lines.Add('')
  $Lines.Add('1. 任务栏和托盘图标均为白色圆角底 DH 图标。')
  $Lines.Add('2. 点击任务栏按钮或最小化按钮时，窗口只最小化到任务栏。')
  $Lines.Add('3. 点击右上角 X 时显示“挂到托盘 / 退出 DH BOT”。')
  $Lines.Add('4. 登录旺商聊后重启 DH BOT 和旺商聊，确认原登录状态继续可用。')
  $Lines.Add('5. 退出 DH BOT 后，旺商聊继续运行，DH BOT 及其 WebView2 子进程消失。')
  $Lines | Set-Content -LiteralPath $MarkdownPath -Encoding utf8
  Write-Host "报告已写入：$MarkdownPath"
}

$NativeSource = @'
using System;
using System.Runtime.InteropServices;
using System.Text;
namespace DhBotRealMachine {
  public static class NativeWindow {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [StructLayout(LayoutKind.Sequential)]
    public struct Rect { public int Left; public int Top; public int Right; public int Bottom; }
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out Rect rect);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int maxCount);
    [DllImport("user32.dll")] private static extern int GetWindowTextLength(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr hWnd, int index);
    [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

    public static string GetWindowTitle(IntPtr hWnd) {
      var text = new StringBuilder(Math.Max(GetWindowTextLength(hWnd) + 1, 2));
      GetWindowText(hWnd, text, text.Capacity);
      return text.ToString();
    }

    public static IntPtr FindMainWindow(int processId) {
      var bestHandle = IntPtr.Zero;
      long bestScore = long.MinValue;
      EnumWindows((handle, _) => {
        GetWindowThreadProcessId(handle, out var ownerProcessId);
        if (ownerProcessId != processId) return true;
        GetWindowRect(handle, out var rect);
        var width = Math.Max(rect.Right - rect.Left, 0);
        var height = Math.Max(rect.Bottom - rect.Top, 0);
        var title = GetWindowTitle(handle);
        if (title.IndexOf("DH BOT", StringComparison.OrdinalIgnoreCase) < 0) return true;
        var style = GetWindowLongPtr(handle, -16).ToInt64();
        if ((style & 0x00CF0000L) != 0x00CF0000L || !IsWindowVisible(handle)) return true;
        long score = (long)width * height;
        if (width < 200 || height < 100) score -= 10000000;
        if (IsWindowVisible(handle)) score += 1000000;
        if (title.Equals("DH BOT", StringComparison.OrdinalIgnoreCase)) score += 100000000;
        else if (title.IndexOf("DH BOT", StringComparison.OrdinalIgnoreCase) >= 0) score += 10000000;
        if (score > bestScore) {
          bestScore = score;
          bestHandle = handle;
        }
        return true;
      }, IntPtr.Zero);
      return bestHandle;
    }
  }
}
'@
Add-Type -TypeDefinition $NativeSource

$script:Results = [System.Collections.Generic.List[object]]::new()
$ArtifactPath = Resolve-FullPath $Artifact
if (-not $OutputDirectory) {
  $OutputDirectory = Join-Path ([Environment]::GetFolderPath('Desktop')) ('DH-BOT-Test-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
}
$OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
$WorkingDirectory = Join-Path $env:TEMP ('dh-bot-real-' + [guid]::NewGuid().ToString('N'))
$Executable = $ArtifactPath

try {
  if ([System.IO.Path]::GetExtension($ArtifactPath) -ieq '.zip') {
    New-Item -ItemType Directory -Force -Path $WorkingDirectory | Out-Null
    Expand-Archive -LiteralPath $ArtifactPath -DestinationPath $WorkingDirectory
    $Files = @(Get-ChildItem -LiteralPath $WorkingDirectory -File)
    $Allowed = @('DH-BOT.exe', 'DH-Manual-ZH.md', 'DH-Manual-ZH.pdf', 'DH-BOT-Default-Rules.json')
    $Unexpected = @($Files | Where-Object { $Allowed -notcontains $_.Name })
    if ($Unexpected.Count -eq 0 -and -not @(Get-ChildItem -LiteralPath $WorkingDirectory -Directory)) {
      Add-Result '便携包文件边界' 'passed' '只包含主程序、手册和默认规则'
    } else {
      Add-Result '便携包文件边界' 'failed' ('出现额外内容：' + (($Unexpected.Name) -join ', '))
    }
    $Executable = Join-Path $WorkingDirectory 'DH-BOT.exe'
  }

  if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
    throw "产物中缺少 DH-BOT.exe：$Executable"
  }

  $Hash = (Get-FileHash -LiteralPath $Executable -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($ExpectedSha256) {
    if ($Hash -eq $ExpectedSha256.Trim().ToLowerInvariant()) {
      Add-Result '主程序 SHA-256' 'passed' $Hash
    } else {
      Add-Result '主程序 SHA-256' 'failed' "实际 $Hash，预期 $ExpectedSha256"
    }
  } else {
    Add-Result '主程序 SHA-256' 'info' $Hash
  }

  $Signature = Get-AuthenticodeSignature -LiteralPath $Executable
  if ($Signature.Status -eq 'Valid') {
    Add-Result 'Authenticode 签名' 'passed' $Signature.SignerCertificate.Subject
  } else {
    Add-Result 'Authenticode 签名' 'info' "状态：$($Signature.Status)；当前个人云盘发行采用未签名产物，首次运行可能显示未知发布者"
  }

  $WebViewRoots = @(${env:ProgramFiles(x86)}, $env:ProgramFiles, $env:LOCALAPPDATA) |
    Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
  $WebView2 = @($WebViewRoots | ForEach-Object {
    Join-Path $_ 'Microsoft\EdgeWebView\Application'
  } | Where-Object { Test-Path -LiteralPath $_ })
  if ($WebView2.Count -gt 0) {
    Add-Result 'WebView2 Runtime' 'passed' ($WebView2 -join '; ')
  } else {
    Add-Result 'WebView2 Runtime' 'failed' '没有在系统目录检测到 WebView2 Runtime'
  }

  $DevToolsBefore = @(
    (Get-HttpProbe 'http://127.0.0.1:9222/json/version'),
    (Get-HttpProbe 'http://127.0.0.1:9222/json/list')
  )
  $DhProcess = $null
  $WindowBeforeMinimize = $null
  $WindowMinimized = $null
  $WindowRestored = $null
  $DescendantIds = @()

  if (-not $SkipLaunch) {
    $ExistingDh = @(Get-Process -Name 'DH-BOT' -ErrorAction SilentlyContinue)
    if ($ExistingDh.Count -gt 0) {
      Add-Result '单实例前置状态' 'warning' "检测到已有 DH-BOT：$($ExistingDh.Id -join ', ')；请先从托盘真正退出后重测"
    } else {
      Add-Result '单实例前置状态' 'passed' '启动前没有残留 DH-BOT 进程'
    }

    $DhProcess = Start-Process -FilePath $Executable -WorkingDirectory (Split-Path -Parent $Executable) -PassThru
    $Deadline = (Get-Date).AddSeconds($StartupTimeoutSeconds)
    $StableWindowHandle = 0L
    $StableWindowSince = Get-Date
    do {
      Start-Sleep -Milliseconds 500
      $DhProcess.Refresh()
      $WindowBeforeMinimize = Get-WindowSnapshot $DhProcess.Id
      $CurrentWindowHandle = if ($WindowBeforeMinimize) { [long]$WindowBeforeMinimize.handle } else { 0L }
      if ($CurrentWindowHandle -ne $StableWindowHandle) {
        $StableWindowHandle = $CurrentWindowHandle
        $StableWindowSince = Get-Date
      }
    } while ((Get-Date) -lt $Deadline -and $DhProcess.HasExited -eq $false -and (-not $WindowBeforeMinimize -or $WindowBeforeMinimize.handle -eq 0))

    while ((Get-Date) -lt $Deadline -and $DhProcess.HasExited -eq $false -and $StableWindowHandle -ne 0 -and ((Get-Date) - $StableWindowSince).TotalSeconds -lt 2) {
      Start-Sleep -Milliseconds 250
      $DhProcess.Refresh()
      $WindowBeforeMinimize = Get-WindowSnapshot $DhProcess.Id
      $CurrentWindowHandle = if ($WindowBeforeMinimize) { [long]$WindowBeforeMinimize.handle } else { 0L }
      if ($CurrentWindowHandle -ne $StableWindowHandle) {
        $StableWindowHandle = $CurrentWindowHandle
        $StableWindowSince = Get-Date
      }
    }

    if ($DhProcess.HasExited) {
      Add-Result 'DH BOT 启动' 'failed' "进程提前退出，ExitCode=$($DhProcess.ExitCode)"
    } elseif ($WindowBeforeMinimize -and $WindowBeforeMinimize.handle -ne 0) {
      Add-Result 'DH BOT 启动' 'passed' "PID $($DhProcess.Id)，窗口：$($WindowBeforeMinimize.title)"
      [void][DhBotRealMachine.NativeWindow]::ShowWindowAsync([IntPtr]$WindowBeforeMinimize.handle, 6)
      Start-Sleep -Seconds 2
      $WindowMinimized = Get-WindowSnapshot $DhProcess.Id $WindowBeforeMinimize.handle
      if ($WindowMinimized -and $WindowMinimized.minimized -and $WindowMinimized.visible) {
        Add-Result '普通最小化' 'passed' '窗口仍可见且处于最小化状态，进程继续运行'
      } else {
        Add-Result '普通最小化' 'failed' ('窗口状态：' + ($WindowMinimized | ConvertTo-Json -Compress))
      }
      [void][DhBotRealMachine.NativeWindow]::ShowWindowAsync([IntPtr]$WindowBeforeMinimize.handle, 9)
      Start-Sleep -Seconds 2
      $WindowRestored = Get-WindowSnapshot $DhProcess.Id $WindowBeforeMinimize.handle
      if ($WindowRestored -and $WindowRestored.visible -and -not $WindowRestored.minimized) {
        Add-Result '窗口恢复' 'passed' '最小化后可正常恢复'
      } else {
        Add-Result '窗口恢复' 'failed' ('窗口状态：' + ($WindowRestored | ConvertTo-Json -Compress))
      }
    } else {
      Add-Result 'DH BOT 启动' 'failed' "${StartupTimeoutSeconds} 秒内没有找到主窗口"
    }

    $DevToolsDeadline = (Get-Date).AddSeconds($StartupTimeoutSeconds)
    do {
      $VersionProbe = Get-HttpProbe 'http://127.0.0.1:9222/json/version'
      $ListProbe = Get-HttpProbe 'http://127.0.0.1:9222/json/list'
      if ($VersionProbe.success -or $ListProbe.success) { break }
      Start-Sleep -Seconds 1
    } while ((Get-Date) -lt $DevToolsDeadline)

    if ($VersionProbe.success -or $ListProbe.success) {
      Add-Result '旺商聊 DevTools 9222' 'passed' "json/version=$($VersionProbe.statusCode)，json/list=$($ListProbe.statusCode)"
    } else {
      Add-Result '旺商聊 DevTools 9222' 'failed' "version=$($VersionProbe.error)；list=$($ListProbe.error)"
    }

    $RuntimeSnapshot = Get-ProcessSnapshot
    $WangProcesses = @($RuntimeSnapshot | Where-Object {
      $_.name -match 'wangshangliao|旺商聊' -or $_.commandLine -match 'remote-debugging-port=9222'
    })
    if ($WangProcesses.Count -gt 0) {
      $WithPort = @($WangProcesses | Where-Object { $_.commandLine -match '(--remote-debugging-port(?:=|\s+)9222)' })
      if ($WithPort.Count -gt 0) {
        Add-Result '旺商聊启动参数' 'passed' (($WithPort | ForEach-Object { "PID $($_.pid) $($_.path)" }) -join '; ')
      } else {
        Add-Result '旺商聊启动参数' 'warning' '找到旺商聊进程，但命令行没有 9222 参数；可能需要在 DH BOT 中确认重启'
      }
    } else {
      Add-Result '旺商聊进程' 'failed' '没有检测到旺商聊进程'
    }

    $DescendantIds = Get-DescendantIds $DhProcess.Id $RuntimeSnapshot

    if ($VerifyInteractiveExit -and -not $DhProcess.HasExited) {
      Write-Host ''
      Write-Host '请现在点击 DH BOT 右上角 X，并在提示中选择“退出 DH BOT”。脚本将等待 90 秒。' -ForegroundColor Cyan
      $DhProcess.WaitForExit(90000) | Out-Null
      Start-Sleep -Seconds 5
      if (Get-Process -Id $DhProcess.Id -ErrorAction SilentlyContinue) {
        Add-Result '真正退出' 'failed' "90 秒后 DH-BOT PID $($DhProcess.Id) 仍存在"
      } else {
        Add-Result '真正退出' 'passed' 'DH-BOT 主进程已退出'
      }
      $RemainingChildren = @($DescendantIds | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
      if ($RemainingChildren.Count -eq 0) {
        Add-Result '退出后子进程清理' 'passed' '启动时属于 DH BOT 的 WebView2/子进程均已退出'
      } else {
        Add-Result '退出后子进程清理' 'failed' ('残留 PID：' + ($RemainingChildren -join ', '))
      }
      $WangAfterExit = @(Get-ProcessSnapshot | Where-Object { $_.name -match 'wangshangliao|旺商聊' })
      if ($WangAfterExit.Count -gt 0) {
        Add-Result '退出不结束旺商聊' 'passed' ('旺商聊 PID：' + (($WangAfterExit.pid) -join ', '))
      } else {
        Add-Result '退出不结束旺商聊' 'warning' '退出后没有检测到旺商聊，请确认是否由旺商聊自身退出'
      }
    } else {
      Add-Result '关闭与进程清理' 'info' '追加 -VerifyInteractiveExit 后，可验证 X 关闭提示和退出残留'
    }
  }

  $Report = [ordered]@{
    schema = 1
    generatedAt = [DateTime]::UtcNow.ToString('o')
    environment = [ordered]@{
      osVersion = [Environment]::OSVersion.VersionString
      edition = (Get-CimInstance Win32_OperatingSystem).Caption
      architecture = $env:PROCESSOR_ARCHITECTURE
      user = [Environment]::UserName
      powershell = $PSVersionTable.PSVersion.ToString()
    }
    artifact = [ordered]@{
      path = $ArtifactPath
      executable = $Executable
      sha256 = $Hash
      size = (Get-Item -LiteralPath $Executable).Length
      signatureStatus = [string]$Signature.Status
    }
    devtoolsBeforeLaunch = $DevToolsBefore
    windows = [ordered]@{
      beforeMinimize = $WindowBeforeMinimize
      minimized = $WindowMinimized
      restored = $WindowRestored
    }
    results = @($script:Results)
  }
  Write-Reports $OutputDirectory $Report

  $Failures = @($script:Results | Where-Object { $_.status -eq 'failed' })
  if ($Failures.Count -gt 0) {
    Write-Host "实机验收发现 $($Failures.Count) 项失败，请查看报告。" -ForegroundColor Red
    exit 1
  }
  Write-Host '实机自动检查通过；请继续完成报告末尾的人工确认项。' -ForegroundColor Green
} finally {
  if (Test-Path -LiteralPath $WorkingDirectory) {
    Remove-Item -LiteralPath $WorkingDirectory -Recurse -Force -ErrorAction SilentlyContinue
  }
}

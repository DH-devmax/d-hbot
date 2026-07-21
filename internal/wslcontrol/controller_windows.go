//go:build windows

package wslcontrol

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"time"
	"unicode/utf16"
	"unsafe"
)

type Windows struct{}

const createNoWindow = 0x08000000

const (
	swShow    = 5
	swRestore = 9
)

var (
	user32                 = syscall.NewLazyDLL("user32.dll")
	procEnumWindows        = user32.NewProc("EnumWindows")
	procGetWindowPID       = user32.NewProc("GetWindowThreadProcessId")
	procGetWindowTextLen   = user32.NewProc("GetWindowTextLengthW")
	procGetWindowText      = user32.NewProc("GetWindowTextW")
	procGetClassName       = user32.NewProc("GetClassNameW")
	procShowWindowAsync    = user32.NewProc("ShowWindowAsync")
	procBringWindowToTop   = user32.NewProc("BringWindowToTop")
	procSetForeground      = user32.NewProc("SetForegroundWindow")
	procAttachThreadInput  = user32.NewProc("AttachThreadInput")
	procGetForeground      = user32.NewProc("GetForegroundWindow")
	procGetWindowThread    = user32.NewProc("GetWindowThreadProcessId")
	procGetCurrentThreadID = syscall.NewLazyDLL("kernel32.dll").NewProc("GetCurrentThreadId")
)

func hideConsole(command *exec.Cmd) {
	command.SysProcAttr = &syscall.SysProcAttr{HideWindow: true, CreationFlags: createNoWindow}
}

func (Windows) Locate(ctx context.Context) ([]InstallCandidate, error) {
	programFiles := []string{os.Getenv("ProgramFiles"), os.Getenv("ProgramFiles(x86)"), os.Getenv("LOCALAPPDATA")}
	paths := []string{}
	for _, root := range programFiles {
		if root == "" {
			continue
		}
		paths = append(paths,
			filepath.Join(root, "wangshangliao_win_online", "wangshangliao_win_online.exe"),
			filepath.Join(root, "WangShangLiao", "WangShangLiao.exe"),
			filepath.Join(root, "旺商聊", "旺商聊.exe"),
		)
	}
	var out []InstallCandidate
	seen := map[string]bool{}
	for _, path := range paths {
		if info, err := os.Stat(path); err == nil && !info.IsDir() {
			path, _ = filepath.Abs(path)
			key := strings.ToLower(filepath.Clean(path))
			if !seen[key] {
				seen[key] = true
				out = append(out, InstallCandidate{Path: path, Source: "常见安装目录"})
			}
		}
	}
	return out, nil
}

func (Windows) Inspect(ctx context.Context, raw string) (DevToolsStatus, error) {
	request, err := httpRequest(ctx, raw+"/json/version")
	if err == nil {
		defer request.Body.Close()
		if request.StatusCode == 200 {
			var value map[string]any
			if json.NewDecoder(request.Body).Decode(&value) == nil {
				if browser, _ := value["User-Agent"].(string); strings.Contains(strings.ToLower(browser), "wangshangliao") {
					return DevToolsReady, nil
				}
				return DevToolsReady, nil
			}
		}
	}
	if !portOpen(ctx, raw) {
		return DevToolsUnavailable, nil
	}
	return DevToolsOtherService, nil
}

func (Windows) ListProcesses(ctx context.Context, imagePath string) ([]ProcessRef, error) {
	command := `$ErrorActionPreference='Stop'; Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -ne $null } | Select-Object ProcessId,ExecutablePath,CreationDate | ConvertTo-Json -Compress`
	raw, err := powershell(ctx, command)
	if err != nil {
		return nil, err
	}
	var rows []struct {
		PID            int    `json:"ProcessId"`
		ExecutablePath string `json:"ExecutablePath"`
		CreationDate   string `json:"CreationDate"`
	}
	if strings.TrimSpace(raw) == "" {
		return nil, nil
	}
	if strings.HasPrefix(strings.TrimSpace(raw), "{") {
		var one struct {
			PID            int    `json:"ProcessId"`
			ExecutablePath string `json:"ExecutablePath"`
			CreationDate   string `json:"CreationDate"`
		}
		if err := json.Unmarshal([]byte(raw), &one); err != nil {
			return nil, err
		}
		rows = append(rows, one)
	} else if err := json.Unmarshal([]byte(raw), &rows); err != nil {
		return nil, err
	}
	want, _ := filepath.Abs(imagePath)
	want = strings.ToLower(filepath.Clean(want))
	var out []ProcessRef
	for _, row := range rows {
		path, _ := filepath.Abs(row.ExecutablePath)
		if strings.ToLower(filepath.Clean(path)) != want {
			continue
		}
		out = append(out, ProcessRef{PID: row.PID, ImagePath: path, StartedAt: parseWMI(string(row.CreationDate))})
	}
	return out, nil
}

func (Windows) Start(ctx context.Context, options StartOptions) (ProcessRef, error) {
	path, err := filepath.Abs(options.ImagePath)
	if err != nil {
		return ProcessRef{}, err
	}
	if _, err := os.Stat(path); err != nil {
		return ProcessRef{}, fmt.Errorf("旺商聊路径不可用：%w", err)
	}
	devtools := options.DevToolsURL
	if devtools == "" {
		devtools = "http://127.0.0.1:9222"
	}
	port := "9222"
	if index := strings.LastIndex(devtools, ":"); index >= 0 {
		port = strings.TrimRight(devtools[index+1:], "/")
	}
	profile := options.ProfileID
	if profile == "" {
		profile = defaultProfileID
	}
	command := exec.CommandContext(ctx, path, "--remote-debugging-port="+port, "--remote-allow-origins=http://127.0.0.1:"+port)
	// WangShangLiao resolves part of its profile/configuration relative to the
	// executable directory. Starting from DH's directory can create a second
	// local profile and make an already logged-in account look like a fresh one.
	command.Dir = filepath.Dir(path)
	command.Env = append(os.Environ(), "DH_WSL_PARTITION="+profile)
	hideConsole(command)
	if err := command.Start(); err != nil {
		return ProcessRef{}, err
	}
	return ProcessRef{PID: command.Process.Pid, ImagePath: path, StartedAt: time.Now()}, nil
}

func (Windows) Activate(ctx context.Context, imagePath string) (ActivationResult, error) {
	processes, err := (Windows{}).ListProcesses(ctx, imagePath)
	if err != nil {
		return ActivationResult{}, err
	}
	if len(processes) == 0 {
		return ActivationResult{}, errors.New("旺商聊进程未运行")
	}
	targets := make(map[uint32]ProcessRef, len(processes))
	for _, process := range processes {
		targets[uint32(process.PID)] = process
	}
	type candidate struct {
		hwnd, pid    uint32
		title, class string
		score        int
	}
	var found []candidate
	callback := syscall.NewCallback(func(hwnd uintptr, _ uintptr) uintptr {
		var pid uint32
		procGetWindowPID.Call(hwnd, uintptr(unsafe.Pointer(&pid)))
		if _, ok := targets[pid]; !ok {
			return 1
		}
		title := windowText(hwnd, procGetWindowTextLen, procGetWindowText)
		className := windowClass(hwnd)
		value := strings.ToLower(title + " " + className)
		score := 1
		if strings.Contains(value, "旺商聊") || strings.Contains(value, "wangshangliao") {
			score += 100
		}
		if strings.Contains(strings.ToLower(className), "chrome_widgetwin") {
			score += 20
		}
		found = append(found, candidate{hwnd: uint32(hwnd), pid: pid, title: title, class: className, score: score})
		return 1
	})
	procEnumWindows.Call(callback, 0)
	if len(found) == 0 {
		return ActivationResult{}, errors.New("旺商聊主窗口尚未创建")
	}
	sort.SliceStable(found, func(i, j int) bool { return found[i].score > found[j].score })
	chosen := found[0]
	hwnd := uintptr(chosen.hwnd)
	procShowWindowAsync.Call(hwnd, swRestore)
	procShowWindowAsync.Call(hwnd, swShow)
	procBringWindowToTop.Call(hwnd)
	foreground, _, _ := procGetForeground.Call()
	currentThread, _, _ := procGetCurrentThreadID.Call()
	foregroundThread, _, _ := procGetWindowThread.Call(foreground, 0)
	targetThread, _, _ := procGetWindowThread.Call(hwnd, 0)
	if foregroundThread != 0 && targetThread != 0 && foregroundThread != currentThread {
		procAttachThreadInput.Call(foregroundThread, currentThread, 1)
		procAttachThreadInput.Call(currentThread, targetThread, 1)
		procSetForeground.Call(hwnd)
		procAttachThreadInput.Call(currentThread, targetThread, 0)
		procAttachThreadInput.Call(foregroundThread, currentThread, 0)
	} else {
		procSetForeground.Call(hwnd)
	}
	return ActivationResult{PID: int(chosen.pid), Window: hwnd, Title: chosen.title, Activated: true}, nil
}

func windowText(hwnd uintptr, lengthProc, textProc *syscall.LazyProc) string {
	length, _, _ := lengthProc.Call(hwnd)
	if length == 0 {
		return ""
	}
	buffer := make([]uint16, length+1)
	textProc.Call(hwnd, uintptr(unsafe.Pointer(&buffer[0])), uintptr(len(buffer)))
	return string(utf16.Decode(buffer))
}

func windowClass(hwnd uintptr) string {
	buffer := make([]uint16, 256)
	procGetClassName.Call(hwnd, uintptr(unsafe.Pointer(&buffer[0])), uintptr(len(buffer)))
	return string(utf16.Decode(buffer))
}

func (Windows) PreparePersistentProfile(ctx context.Context, imagePath, profileID string) (ProfileStatus, error) {
	status, err := preparePersistentProfileDirect(ctx, imagePath, profileID)
	if err == nil || status.State != "permission" || os.Getenv("DH_PROFILE_HELPER") == "1" {
		return status, err
	}
	if elevateErr := elevateProfilePatch(ctx, imagePath, profileID); elevateErr != nil {
		return status, fmt.Errorf("需要管理员权限修改旺商聊启动脚本：%w", elevateErr)
	}
	return preparePersistentProfileDirect(ctx, imagePath, profileID)
}

func preparePersistentProfileDirect(ctx context.Context, imagePath, profileID string) (ProfileStatus, error) {
	if profileID == "" {
		profileID = defaultProfileID
	}
	scriptPath := filepath.Join(filepath.Dir(imagePath), "resources", "app", "dist-electron", "main", "index.js")
	if _, err := os.Stat(scriptPath); err != nil {
		return ProfileStatus{State: "unsupported", ScriptPath: scriptPath, ProfileID: profileID, Detail: "旺商聊主脚本未找到"}, err
	}
	original, err := os.ReadFile(scriptPath)
	if err != nil {
		return ProfileStatus{}, err
	}
	originalSHA := fmt.Sprintf("%x", sha256.Sum256(original))
	if strings.Contains(string(original), fixedPartitionCode) {
		backupPath := matchingProfileBackup(filepath.Join(defaultDHConfigDir(), "wsl-backups"), original)
		status := ProfileStatus{State: "enabled", ScriptPath: scriptPath, BackupPath: backupPath, ProfileRoot: wangShangLiaoProfileRoot(), ProfileID: profileID, OriginalSHA: originalSHA, PatchedSHA: originalSHA, Detail: "固定登录分区已生效"}
		status.Migrated = migrateLatestPartition(status.ProfileRoot, profileID)
		return status, nil
	}
	patched, changed, patchErr := patchProfileScript(original)
	if patchErr != nil {
		return ProfileStatus{State: "unsupported", ScriptPath: scriptPath, ProfileRoot: wangShangLiaoProfileRoot(), ProfileID: profileID, OriginalSHA: originalSHA, Detail: "旺商聊版本未匹配固定分区补丁"}, errors.New("旺商聊启动脚本版本不匹配")
	}
	if !changed {
		return ProfileStatus{State: "enabled", ScriptPath: scriptPath, ProfileRoot: wangShangLiaoProfileRoot(), ProfileID: profileID, OriginalSHA: originalSHA, PatchedSHA: originalSHA, Detail: "固定登录分区已生效"}, nil
	}
	backupDir := filepath.Join(defaultDHConfigDir(), "wsl-backups")
	if err := os.MkdirAll(backupDir, 0o700); err != nil {
		return ProfileStatus{}, err
	}
	backupPath := filepath.Join(backupDir, "wangshangliao-"+originalSHA+".index.js")
	if _, err := os.Stat(backupPath); errors.Is(err, os.ErrNotExist) {
		if err := os.WriteFile(backupPath, original, 0o600); err != nil {
			return ProfileStatus{}, err
		}
	}
	tmp := scriptPath + ".dh-tmp"
	if err := os.WriteFile(tmp, patched, 0o600); err != nil {
		return ProfileStatus{State: "permission", ScriptPath: scriptPath, BackupPath: backupPath, ProfileID: profileID, Detail: "需要管理员权限修改旺商聊启动脚本"}, err
	}
	if err := os.Remove(scriptPath); err != nil {
		_ = os.Remove(tmp)
		return ProfileStatus{State: "permission", ScriptPath: scriptPath, BackupPath: backupPath, ProfileID: profileID, Detail: "需要管理员权限替换旺商聊启动脚本"}, err
	}
	if err := os.Rename(tmp, scriptPath); err != nil {
		_ = os.WriteFile(scriptPath, original, 0o600)
		_ = os.Remove(tmp)
		return ProfileStatus{}, err
	}
	patchedSHA := fmt.Sprintf("%x", sha256.Sum256(patched))
	status := ProfileStatus{State: "enabled", ScriptPath: scriptPath, BackupPath: backupPath, ProfileRoot: wangShangLiaoProfileRoot(), ProfileID: profileID, OriginalSHA: originalSHA, PatchedSHA: patchedSHA, Detail: "固定登录分区已启用"}
	status.Migrated = migrateLatestPartition(status.ProfileRoot, profileID)
	return status, nil
}

func RunProfileHelper(args []string) error {
	if len(args) < 2 {
		return errors.New("旺商聊登录分区维护参数不完整")
	}
	_ = os.Setenv("DH_PROFILE_HELPER", "1")
	_, err := preparePersistentProfileDirect(context.Background(), args[0], args[1])
	return err
}

func RunProfileRestoreHelper(args []string) error {
	if len(args) < 2 {
		return errors.New("旺商聊原文件恢复参数不完整")
	}
	_ = os.Setenv("DH_PROFILE_HELPER", "1")
	return restoreProfileScript(args[0], args[1])
}

func elevateProfilePatch(ctx context.Context, imagePath, profileID string) error {
	executable, err := os.Executable()
	if err != nil {
		return err
	}
	script := fmt.Sprintf("$p=Start-Process -Verb RunAs -WindowStyle Hidden -FilePath '%s' -ArgumentList @('--dh-profile-helper','%s','%s') -Wait -PassThru; exit $p.ExitCode", quotePowerShell(executable), quotePowerShell(imagePath), quotePowerShell(profileID))
	command := exec.CommandContext(ctx, "powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script)
	hideConsole(command)
	output, err := command.CombinedOutput()
	if err != nil {
		return fmt.Errorf("UAC 维护进程失败：%w：%s", err, strings.TrimSpace(string(output)))
	}
	return nil
}

func quotePowerShell(value string) string {
	return strings.ReplaceAll(value, "'", "''")
}

func (Windows) RestorePersistentProfile(ctx context.Context, scriptPath, backupPath string) error {
	err := restoreProfileScript(scriptPath, backupPath)
	if err == nil || os.Getenv("DH_PROFILE_HELPER") == "1" || !os.IsPermission(err) {
		return err
	}
	return elevateProfileRestore(ctx, scriptPath, backupPath)
}

func elevateProfileRestore(ctx context.Context, scriptPath, backupPath string) error {
	executable, err := os.Executable()
	if err != nil {
		return err
	}
	script := fmt.Sprintf("$p=Start-Process -Verb RunAs -WindowStyle Hidden -FilePath '%s' -ArgumentList @('--dh-profile-restore-helper','%s','%s') -Wait -PassThru; exit $p.ExitCode", quotePowerShell(executable), quotePowerShell(scriptPath), quotePowerShell(backupPath))
	command := exec.CommandContext(ctx, "powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script)
	hideConsole(command)
	output, err := command.CombinedOutput()
	if err != nil {
		return fmt.Errorf("UAC 恢复进程失败：%w：%s", err, strings.TrimSpace(string(output)))
	}
	return nil
}

func defaultDHConfigDir() string {
	base, err := os.UserConfigDir()
	if err != nil || base == "" {
		base = os.Getenv("APPDATA")
	}
	if base == "" {
		base = "."
	}
	return filepath.Join(base, "DH")
}

func wangShangLiaoProfileRoot() string {
	base := os.Getenv("APPDATA")
	if base == "" {
		base, _ = os.UserConfigDir()
	}
	return filepath.Join(base, "wangshangliao")
}

func (Windows) Stop(ctx context.Context, process ProcessRef) error {
	if process.PID <= 0 || process.ImagePath == "" {
		return errors.New("旺商聊进程信息不完整")
	}
	rows, err := (Windows{}).ListProcesses(ctx, process.ImagePath)
	if err != nil {
		return err
	}
	found := false
	for _, row := range rows {
		if row.PID == process.PID {
			found = true
			break
		}
	}
	if !found {
		return nil
	}
	if _, err := powershell(ctx, fmt.Sprintf(`Stop-Process -Id %d`, process.PID)); err != nil {
		return err
	}
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		current, listErr := (Windows{}).ListProcesses(ctx, process.ImagePath)
		if listErr != nil || !containsPID(current, process.PID) {
			return nil
		}
		time.Sleep(200 * time.Millisecond)
	}
	_, err = powershell(ctx, fmt.Sprintf(`Stop-Process -Id %d -Force`, process.PID))
	return err
}

func containsPID(processes []ProcessRef, pid int) bool {
	for _, process := range processes {
		if process.PID == pid {
			return true
		}
	}
	return false
}

func powershell(ctx context.Context, script string) (string, error) {
	command := exec.CommandContext(ctx, "powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script)
	hideConsole(command)
	output, err := command.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("Windows 进程查询失败：%w：%s", err, strings.TrimSpace(string(output)))
	}
	return strings.TrimSpace(string(output)), nil
}

func parseWMI(value string) time.Time {
	value = strings.TrimSpace(value)
	if len(value) < 14 {
		return time.Time{}
	}
	parsed, _ := time.ParseInLocation("20060102150405", value[:14], time.Local)
	return parsed
}

// Keep these small helpers local so the controller has no dependency on the
// UI's protocol client and can be unit-tested with a loopback fixture.
type httpResponse struct {
	Body       io.ReadCloser
	StatusCode int
}

func httpRequest(ctx context.Context, raw string) (httpResponse, error) {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, strings.TrimRight(raw, "/"), nil)
	if err != nil {
		return httpResponse{}, err
	}
	response, err := http.DefaultClient.Do(request)
	if err != nil {
		return httpResponse{}, err
	}
	return httpResponse{Body: response.Body, StatusCode: response.StatusCode}, nil
}

func portOpen(ctx context.Context, raw string) bool {
	command := exec.CommandContext(ctx, "powershell.exe", "-NoProfile", "-NonInteractive", "-Command", fmt.Sprintf(`try { $r=Invoke-WebRequest -UseBasicParsing -TimeoutSec 2 '%s/json/version'; exit 0 } catch { exit 1 }`, strings.TrimRight(raw, "/")))
	hideConsole(command)
	return command.Run() == nil
}

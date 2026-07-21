//go:build windows

package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"
	"unsafe"

	"dh/internal/aiprovider"
	"dh/internal/appcore"
	"dh/internal/diagnostics"
	"dh/internal/embeddedbridge"
	"dh/internal/groupmgr"
	"dh/internal/knowledge"
	"dh/internal/migration"
	"dh/internal/moderation"
	"dh/internal/protocol"
	"dh/internal/secrets"
	"dh/internal/store"
	"dh/internal/urlguard"
	"dh/internal/wslcontrol"
)

const (
	wmNull         = 0x0000
	wmCreate       = 0x0001
	wmDestroy      = 0x0002
	wmSize         = 0x0005
	wmGetMinMax    = 0x0024
	wmPaint        = 0x000F
	wmEraseBkgnd   = 0x0014
	wmTimer        = 0x0113
	wmCommand      = 0x0111
	wmSysCommand   = 0x0112
	wmNotify       = 0x004E
	wmSetRedraw    = 0x000B
	wmLButtonDown  = 0x0201
	wmLButtonUp    = 0x0202
	wmLButtonDbl   = 0x0203
	wmRButtonUp    = 0x0205
	wmContextMenu  = 0x007B
	wmMouseMove    = 0x0200
	wmMouseLeave   = 0x02A3
	wmSetFont      = 0x0030
	wmClose        = 0x0010
	wmAppRefresh   = 0x8001
	wmAppTray      = 0x8002
	wmAppTrayProbe = 0x8003
	wmAppShow      = 0x8004
	wmAITestResult = 0x8010

	wsOverlapped            = 0x00000000
	wsCaption               = 0x00C00000
	wsSysMenu               = 0x00080000
	wsThickFrame            = 0x00040000
	wsMinimizeBox           = 0x00020000
	wsMaximizeBox           = 0x00010000
	wsVisible               = 0x10000000
	wsChild                 = 0x40000000
	wsClipChildren          = 0x02000000
	wsBorder                = 0x00800000
	wsTabStop               = 0x00010000
	esAutoHScroll           = 0x0080
	esMultiLine             = 0x0004
	esAutoVScroll           = 0x0040
	esWantReturn            = 0x1000
	esPassword              = 0x0020
	esReadOnly              = 0x0800
	bsAutoCheckbox          = 0x0003
	bmGetCheck              = 0x00F0
	bmSetCheck              = 0x00F1
	bstChecked              = 1
	swHide                  = 0
	swShow                  = 5
	swRestore               = 9
	sizeMinimized           = 1
	scMinimize              = 0xF020
	colorWindow             = 5
	idcArrow                = 32512
	idiApp                  = 2
	dtLeft                  = 0x0000
	dtCenter                = 0x0001
	dtRight                 = 0x0002
	dtVCenter               = 0x0004
	dtSingleLine            = 0x0020
	dtEndEllipsis           = 0x8000
	dtWordBreak             = 0x0010
	transparent             = 1
	mbYesNo                 = 0x00000004
	mbIconWarning           = 0x00000030
	mbIconError             = 0x00000010
	mbOK                    = 0x00000000
	idYes                   = 6
	psSolid                 = 0
	wsExClientEdge          = 0x00000200
	lvsReport               = 0x0001
	lvsSingleSel            = 0x0004
	lvsShowSelAlways        = 0x0008
	lvmFirst                = 0x1000
	lvmDeleteAllItems       = lvmFirst + 9
	lvmGetItemCount         = lvmFirst + 4
	lvmGetNextItem          = lvmFirst + 12
	lvmGetItemState         = lvmFirst + 44
	lvmSetExtendedStyle     = lvmFirst + 54
	lvmGetItemW             = lvmFirst + 75
	lvmInsertItemW          = lvmFirst + 77
	lvmSetItemW             = lvmFirst + 76
	lvmInsertColumnW        = lvmFirst + 97
	lvmSetItemState         = lvmFirst + 43
	lvifText                = 0x0001
	lvifParam               = 0x0004
	lvcfFmt                 = 0x0001
	lvcfWidth               = 0x0002
	lvcfText                = 0x0004
	lvcfSubItem             = 0x0008
	lvisFocused             = 0x0001
	lvisSelected            = 0x0002
	lvisStateImageMask      = 0xF000
	lvniSelected            = 0x0002
	lvsExFullRowSelect      = 0x00000020
	lvsExCheckboxes         = 0x00000004
	lvsExDoubleBuffer       = 0x00010000
	iccListViewClasses      = 0x00000001
	iccDateClasses          = 0x00000100
	dtsShowNone             = 0x0002
	dtsTimeFormat           = 0x0009
	dtmFirst                = 0x1000
	dtmGetSystemTime        = dtmFirst + 1
	dtmSetSystemTime        = dtmFirst + 2
	dtmSetFormatW           = dtmFirst + 50
	gdtValid                = 0
	gdtNone                 = 1
	tmeLeave                = 0x00000002
	nimAdd                  = 0x00000000
	nimSetVersion           = 0x00000004
	nimDelete               = 0x00000002
	nifMessage              = 0x00000001
	nifIcon                 = 0x00000002
	nifTip                  = 0x00000004
	mfSeparator             = 0x00000800
	tpmRightButton          = 0x00000002
	tpmReturnCmd            = 0x00000100
	trayOpenCommand         = 41001
	trayPauseCommand        = 41002
	trayExitCommand         = 41003
	sidebarNavTop           = 78
	minWindowWidth          = 1180
	minWindowHeight         = 760
	localKnowledgeAccountID = "__local__"
)

const (
	pageOverview = iota
	pageGroups
	pageMessages
	pageRules
	pageKnowledge
	pageTasks
	pageAudit
	pageSettings
	pageDebug
)

const (
	editWelcome         = 2001
	editSummarySchedule = 2002
	editMuteMinutes     = 2003
	editCardPrefix      = 2004
	editCardSuggestion  = 2005

	editMessageGroup = 2101
	editMessageText  = 2102

	editRuleGroup   = 2201
	editRulePattern = 2202
	editRulePath    = 2204

	editKnowledgeGroup   = 2301
	editKnowledgeTitle   = 2302
	editKnowledgePath    = 2303
	editKnowledgeContent = 2304
	editKnowledgeBase    = 2305
	editKnowledgeDesc    = 2306

	editTaskGroup    = 2401
	editTaskTitle    = 2402
	editTaskAssignee = 2403
	editTaskDue      = 2404

	editDevTools      = 2501
	editEndpoint      = 2502
	editWakeWords     = 2503
	editAIKind        = 2504
	editAIBase        = 2505
	editAIModel       = 2506
	editAIKey         = 2507
	editWebhookURL    = 2508
	editWangPath      = 2509
	editScheduleName  = 2510
	editScheduleOpen  = 2511
	editScheduleClose = 2512
	editSearch        = 2601
	checkRuleRecall   = 2701
	checkRuleMute     = 2702
	checkAutoStart    = 2703
	checkPersistLogin = 2704
	checkSchedule     = 2705

	listOverview  = 3001
	listGroups    = 3002
	listMembers   = 3003
	listMessages  = 3004
	listRules     = 3005
	listKnowledge = 3006
	listTasks     = 3007
	listAudit     = 3008
	listSummaries = 3009
	listSchedules = 3010
)

var navLabels = []string{"总览", "群组与成员", "消息台", "规则", "知识与 AI", "任务与摘要", "审计", "设置", "调试"}
var pageTitles = navLabels

var auditEventLabels = map[string]string{
	"inactive_member_cleanup_failed":  "清理封禁成员失败",
	"inactive_member_cleanup_removed": "已清理封禁成员",
	"card_original_restored":          "已恢复原群名片",
	"card_batch_queued":               "群名片批量任务已排队",
	"card_prefix_changed":             "群名片前缀已修改",
	"card_rename_permission_paused":   "群名片任务因权限暂停",
	"card_rename_inactive_skipped":    "已跳过失效成员改名",
	"card_rename_verified":            "群名片修改已验证",
	"card_rename_failed":              "群名片修改失败",
	"card_rename_retry":               "群名片修改将重试",
	"manager_permission_lost":         "管理权限已失效",
	"welcome_skipped":                 "已跳过欢迎语",
	"welcome_error":                   "欢迎语发送失败",
	"welcome":                         "已发送欢迎语",
	"nickname_restored":               "已恢复锁定群名片",
	"ai_queue_full":                   "AI 任务队列已满",
	"ai_error":                        "AI 处理失败",
	"ai_decision":                     "AI 处理决策",
	"ai_test":                         "AI 连接测试",
	"summary_error":                   "每日摘要生成失败",
	"summary":                         "每日摘要已生成",
	"task_reminder_error":             "任务提醒发送失败",
	"task_reminder":                   "已发送任务提醒",
	"manual_reply":                    "手动发送文本",
	"group_schedule_group_mute":       "定时关群",
	"group_schedule_unmute":           "定时开群",
	"group_schedule_error":            "群发言计划执行失败",
	"prediction_fallback":             "预测已使用本地统计回复",
}

var localizedUserText = map[string]string{
	"message is empty":                   "消息内容为空",
	"AI provider is not configured":      "AI 服务尚未配置",
	"group has no messages to summarize": "当前群暂无可供摘要的消息",
	"AI returned an empty summary":       "AI 未返回摘要内容",
	"AI response has no choice":          "AI 响应中没有可用结果",
	"AI queue reached 256 jobs":          "AI 任务队列已满（256 条）",
	"protocol transport error":           "协议连接错误",
	"protocol permission denied":         "协议权限不足",
	"protocol business error":            "协议业务错误",
	"group gateway: permission denied":   "群管理权限不足",
	"group gateway: business error":      "群管理接口返回业务错误",
	"group gateway: not found":           "群管理对象不存在",
	"manual":                             "手动操作",
	"on":                                 "开启",
	"off":                                "关闭",
}

var userTextReplacer = strings.NewReplacer(
	"AI request HTTP", "AI 请求返回 HTTP",
	"AI request:", "AI 请求失败：",
	"webhook request:", "Webhook 请求失败：",
	"decode AI decision", "解析 AI 返回内容失败",
	"protocol transport error", "协议连接错误",
	"protocol permission denied", "协议权限不足",
	"protocol business error", "协议业务错误",
	"NIM send timeout", "NIM 发送超时",
	"NIM 未就绪：请先登录旺商聊并等待会话初始化", "NIM 尚未初始化，请先登录旺商聊并等待会话初始化",
	"NIM team members timeout", "NIM 群成员读取超时",
	"NIM update nick timeout", "NIM 群名片修改超时",
	"IPC timeout", "IPC 通信超时",
)

const (
	defaultAIBaseURL   = ""
	defaultAIModel     = "deepseek-v4-pro"
	secretPlaceholder  = "[已保存的密钥]"
	defaultWangProfile = "dh-primary"
)

var (
	user32                   = syscall.NewLazyDLL("user32.dll")
	gdi32                    = syscall.NewLazyDLL("gdi32.dll")
	kernel32                 = syscall.NewLazyDLL("kernel32.dll")
	comctl32                 = syscall.NewLazyDLL("comctl32.dll")
	shell32                  = syscall.NewLazyDLL("shell32.dll")
	procRegisterClassExW     = user32.NewProc("RegisterClassExW")
	procCreateWindowExW      = user32.NewProc("CreateWindowExW")
	procDefWindowProcW       = user32.NewProc("DefWindowProcW")
	procDestroyWindow        = user32.NewProc("DestroyWindow")
	procShowWindow           = user32.NewProc("ShowWindow")
	procSetForegroundWindow  = user32.NewProc("SetForegroundWindow")
	procUpdateWindow         = user32.NewProc("UpdateWindow")
	procGetMessageW          = user32.NewProc("GetMessageW")
	procTranslateMessage     = user32.NewProc("TranslateMessage")
	procDispatchMessageW     = user32.NewProc("DispatchMessageW")
	procPostQuitMessage      = user32.NewProc("PostQuitMessage")
	procBeginPaint           = user32.NewProc("BeginPaint")
	procEndPaint             = user32.NewProc("EndPaint")
	procGetClientRect        = user32.NewProc("GetClientRect")
	procInvalidateRect       = user32.NewProc("InvalidateRect")
	procSetWindowPos         = user32.NewProc("SetWindowPos")
	procSetWindowTextW       = user32.NewProc("SetWindowTextW")
	procGetWindowTextW       = user32.NewProc("GetWindowTextW")
	procGetWindowTextLenW    = user32.NewProc("GetWindowTextLengthW")
	procSendMessageW         = user32.NewProc("SendMessageW")
	procPostMessageW         = user32.NewProc("PostMessageW")
	procSetTimer             = user32.NewProc("SetTimer")
	procKillTimer            = user32.NewProc("KillTimer")
	procLoadCursorW          = user32.NewProc("LoadCursorW")
	procLoadIconW            = user32.NewProc("LoadIconW")
	procDrawIconEx           = user32.NewProc("DrawIconEx")
	procGetCursorPos         = user32.NewProc("GetCursorPos")
	procTrackMouseEvent      = user32.NewProc("TrackMouseEvent")
	procCreatePopupMenu      = user32.NewProc("CreatePopupMenu")
	procAppendMenuW          = user32.NewProc("AppendMenuW")
	procCheckMenuItem        = user32.NewProc("CheckMenuItem")
	procTrackPopupMenu       = user32.NewProc("TrackPopupMenu")
	procDestroyMenu          = user32.NewProc("DestroyMenu")
	procMessageBoxW          = user32.NewProc("MessageBoxW")
	procSetProcessDPI        = user32.NewProc("SetProcessDpiAwarenessContext")
	procFindWindowW          = user32.NewProc("FindWindowW")
	procFlashWindowEx        = user32.NewProc("FlashWindowEx")
	procFillRect             = user32.NewProc("FillRect")
	procFrameRect            = user32.NewProc("FrameRect")
	procDrawTextW            = user32.NewProc("DrawTextW")
	procGetModuleHandleW     = kernel32.NewProc("GetModuleHandleW")
	procCreateMutexW         = kernel32.NewProc("CreateMutexW")
	procCloseHandle          = kernel32.NewProc("CloseHandle")
	procCreateSolidBrush     = gdi32.NewProc("CreateSolidBrush")
	procDeleteObject         = gdi32.NewProc("DeleteObject")
	procCreatePen            = gdi32.NewProc("CreatePen")
	procSelectObject         = gdi32.NewProc("SelectObject")
	procSetTextColor         = gdi32.NewProc("SetTextColor")
	procSetBkMode            = gdi32.NewProc("SetBkMode")
	procCreateCompatibleDC   = gdi32.NewProc("CreateCompatibleDC")
	procCreateCompatibleBmp  = gdi32.NewProc("CreateCompatibleBitmap")
	procDeleteDC             = gdi32.NewProc("DeleteDC")
	procBitBlt               = gdi32.NewProc("BitBlt")
	procCreateFontW          = gdi32.NewProc("CreateFontW")
	procRoundRect            = gdi32.NewProc("RoundRect")
	procInitCommonControlsEx = comctl32.NewProc("InitCommonControlsEx")
	procShellNotifyIconW     = shell32.NewProc("Shell_NotifyIconW")
)

type point struct{ X, Y int32 }
type rect struct{ Left, Top, Right, Bottom int32 }
type flashWindowInfo struct {
	Size    uint32
	HWnd    uintptr
	Flags   uint32
	Count   uint32
	Timeout uint32
}
type minMaxInfo struct {
	Reserved, MaxSize, MaxPosition, MinTrackSize, MaxTrackSize point
}
type notifyIconData struct {
	Size             uint32
	HWnd             uintptr
	ID               uint32
	Flags            uint32
	CallbackMessage  uint32
	Icon             uintptr
	Tip              [128]uint16
	State            uint32
	StateMask        uint32
	Info             [256]uint16
	TimeoutOrVersion uint32
	InfoTitle        [64]uint16
	InfoFlags        uint32
	GUID             [16]byte
	BalloonIcon      uintptr
}
type systemTime struct {
	Year, Month, DayOfWeek, Day, Hour, Minute, Second, Milliseconds uint16
}
type trackMouseEvent struct {
	Size      uint32
	Flags     uint32
	HWndTrack uintptr
	HoverTime uint32
}
type msg struct {
	HWnd, Message, WParam, LParam uintptr
	Time                          uint32
	Pt                            point
	Private                       uint32
}
type paintStruct struct {
	HDC       uintptr
	Erase     int32
	Paint     rect
	Restore   int32
	IncUpdate int32
	Reserved  [32]byte
}
type wndClassEx struct {
	Size       uint32
	Style      uint32
	WndProc    uintptr
	ClsExtra   int32
	WndExtra   int32
	Instance   uintptr
	Icon       uintptr
	Cursor     uintptr
	Background uintptr
	MenuName   *uint16
	ClassName  *uint16
	IconSmall  uintptr
}

type initCommonControlsEx struct {
	Size uint32
	ICC  uint32
}

type listViewColumn struct {
	Mask         uint32
	Format       int32
	Width        int32
	Text         *uint16
	TextMax      int32
	SubItem      int32
	Image        int32
	Order        int32
	MinWidth     int32
	DefaultWidth int32
	IdealWidth   int32
}

type listViewItem struct {
	Mask      uint32
	Item      int32
	SubItem   int32
	State     uint32
	StateMask uint32
	Text      *uint16
	TextMax   int32
	Image     int32
	Param     uintptr
	Indent    int32
	GroupID   int32
	Columns   uint32
	ColumnPtr *uint32
	Formats   *int32
	Group     int32
}

type listRow struct {
	ID     int64
	Values []string
}

type debugSnapshot struct {
	CheckedAt    time.Time
	DevTools     string
	DevToolsInfo string
	Bridge       string
	BridgeInfo   string
	Session      string
	SessionInfo  string
	Process      string
	ProcessInfo  string
	Profile      string
	ProfileInfo  string
	Advice       string
}

type uiApp struct {
	hwnd            uintptr
	width, height   int32
	page            int
	edits           map[int]uintptr
	lists           map[int]uintptr
	listPages       map[int]int
	listTotals      map[int]int
	listSignatures  map[int]string
	searches        map[int]string
	fontBody        uintptr
	fontMedium      uintptr
	fontHeading     uintptr
	fontHero        uintptr
	icon            uintptr
	trayIcon        uintptr
	brushes         map[uint32]uintptr
	pens            map[uint32]uintptr
	backDC          uintptr
	backBitmap      uintptr
	backOldBitmap   uintptr
	backWidth       int32
	backHeight      int32
	summaryTime     uintptr
	ruleRecall      uintptr
	ruleMute        uintptr
	autoStart       uintptr
	persistLogin    uintptr
	scheduleEnabled uintptr

	database    *store.Store
	secretStore secrets.Store
	client      *protocol.Client
	gateway     *protocol.HTTPGateway
	manager     *appcore.Manager
	accountID   string
	senderID    int64
	legacyGroup int64

	selectedGroup         int64
	selectedMember        int64
	selectedMembers       map[int64]bool
	groupTab              int
	selectedRule          int64
	selectedTask          int64
	selectedSummary       int64
	selectedKnowledgeBase int64
	knowledgeBases        []groupmgr.KnowledgeBase
	wslController         wslcontrol.Controller
	wslPrompted           bool
	wslActivatedNIM       bool
	wslPath               string
	wslProfileStatus      wslcontrol.ProfileStatus
	pageGroups            map[int]map[int64]bool
	groupSelectionReady   bool

	bridgeService *embeddedbridge.Service
	stop          chan struct{}
	stopOnce      sync.Once
	workerWG      sync.WaitGroup
	apiKeyStored  bool

	mu                sync.RWMutex
	status            string
	detail            string
	online            bool
	busy              bool
	activeAction      string
	animationUntil    time.Time
	animationPhase    int
	lastConnect       time.Time
	lastMemberSync    time.Time
	lastScheduled     time.Time
	cardWorkerRunning bool
	cardPreviewGroup  int64
	cardPreview       map[int64]groupmgr.CardPlan
	cardOverrides     map[int64]string
	debug             debugSnapshot
	hoverText         string
	hoverX            int32
	hoverY            int32
	mouseInside       bool
	mouseDown         bool
	layoutPage        int
	layoutGroupTab    int
	layoutReady       bool
	trayAdded         bool
	trayHidden        bool
	exitRequested     bool
}

type aiTestDialog struct {
	hwnd, promptLabel, prompt, responseLabel, response, status, send, clear, close uintptr
	app                                                                            *uiApp
	mu                                                                             sync.Mutex
	busy                                                                           bool
	history                                                                        []aiprovider.ContextMessage
	pendingReply                                                                   string
	pendingStatus                                                                  string
}

var app *uiApp
var activeAITestDialog *aiTestDialog

var taskbarCreatedMessage uintptr

const singleInstanceName = "Local\\DH.BOT.Native.Desktop.2.7.0"

func main() {
	if len(os.Args) >= 2 && os.Args[1] == "--dh-profile-helper" {
		if err := wslcontrol.RunProfileHelper(os.Args[2:]); err != nil {
			showStartupFailure("旺商聊登录状态维护失败", err.Error())
		}
		return
	}
	if len(os.Args) >= 2 && os.Args[1] == "--dh-profile-restore-helper" {
		if err := wslcontrol.RunProfileRestoreHelper(os.Args[2:]); err != nil {
			showStartupFailure("旺商聊原文件恢复失败", err.Error())
		}
		return
	}
	runtime.LockOSThread()
	defer func() {
		if recovered := recover(); recovered != nil {
			path := writeCrashLog("主线程异常", fmt.Sprint(recovered))
			showStartupFailure("DH BOT 运行中出现异常", path)
		}
	}()
	_, _, _ = procSetProcessDPI.Call(^uintptr(3))
	mutex, existing, err := acquireSingleInstance()
	if err != nil {
		path := writeCrashLog("单实例初始化失败", err.Error())
		showStartupFailure("DH BOT 启动失败", path)
		return
	}
	if existing {
		activateExistingWindow()
		if mutex != 0 {
			procCloseHandle.Call(mutex)
		}
		return
	}
	defer procCloseHandle.Call(mutex)
	controls := initCommonControlsEx{Size: uint32(unsafe.Sizeof(initCommonControlsEx{})), ICC: iccListViewClasses | iccDateClasses}
	procInitCommonControlsEx.Call(uintptr(unsafe.Pointer(&controls)))
	database, err := store.Open(store.DefaultPath())
	if err != nil {
		path := writeCrashLog("数据库启动失败", err.Error())
		showStartupFailure("DH BOT 数据库检查失败", path)
		return
	}
	_ = appcore.EnsureDefaultRulesTemplate(context.Background(), database)
	_, _ = database.EnsureDefaultKnowledgeBase(context.Background(), localKnowledgeAccountID)
	app = &uiApp{
		width: 1320, height: 840, page: pageOverview, edits: make(map[int]uintptr), lists: make(map[int]uintptr),
		listPages: make(map[int]int), listTotals: make(map[int]int), listSignatures: make(map[int]string), searches: make(map[int]string),
		pageGroups: make(map[int]map[int64]bool), selectedMembers: make(map[int64]bool),
		cardPreview: make(map[int64]groupmgr.CardPlan), cardOverrides: make(map[int64]string),
		brushes: make(map[uint32]uintptr), pens: make(map[uint32]uintptr),
		database: database, secretStore: secrets.Store{Path: secrets.DefaultPath()},
		status: "等待旺商聊", stop: make(chan struct{}),
	}
	app.wslProfileStatus = wslcontrol.ProfileStatus{
		State:      firstNonEmpty(app.setting("wsl.profile_state", ""), ""),
		Detail:     app.setting("wsl.profile_status", ""),
		ScriptPath: app.setting("wsl.profile_script", ""),
		BackupPath: app.setting("wsl.profile_backup", ""),
		ProfileID:  firstNonEmpty(app.setting("wsl.profile_id", ""), defaultWangProfile),
	}
	if runtime.GOOS == "windows" {
		app.wslController = wslcontrol.Windows{}
	}
	app.loadLegacy()
	app.createFonts()
	app.startBridge()
	if err := app.run(); err != nil {
		_ = database.Close()
		path := writeCrashLog("窗口启动失败", err.Error())
		showStartupFailure("DH BOT 窗口启动失败", path)
	}
}

func acquireSingleInstance() (uintptr, bool, error) {
	handle, _, callErr := procCreateMutexW.Call(0, 1, uintptr(unsafe.Pointer(utf16(singleInstanceName))))
	if handle == 0 {
		return 0, false, fmt.Errorf("创建单实例互斥量失败：%v", callErr)
	}
	errno, _ := callErr.(syscall.Errno)
	return handle, errno == 183, nil
}

func activateExistingWindow() {
	className := utf16("DHNativeWindowV2")
	hwnd, _, _ := procFindWindowW.Call(uintptr(unsafe.Pointer(className)), 0)
	if hwnd == 0 {
		return
	}
	procPostMessageW.Call(hwnd, wmAppShow, 0, 0)
	flash := flashWindowInfo{Size: uint32(unsafe.Sizeof(flashWindowInfo{})), HWnd: hwnd, Flags: 3, Count: 2}
	procFlashWindowEx.Call(uintptr(unsafe.Pointer(&flash)))
}

func showStartupFailure(message, logPath string) {
	detail := message
	if logPath != "" {
		detail += "\n\n诊断日志：" + logPath
	}
	procMessageBoxW.Call(0, uintptr(unsafe.Pointer(utf16(detail))), uintptr(unsafe.Pointer(utf16("DH BOT"))), mbOK|mbIconError)
}

func writeCrashLog(kind, detail string) string {
	base, err := os.UserConfigDir()
	if err != nil || base == "" {
		base = "."
	}
	dir := filepath.Join(base, "DH", "logs")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return ""
	}
	stack := make([]byte, 64*1024)
	stack = stack[:runtime.Stack(stack, false)]
	content := fmt.Sprintf("时间：%s\n类型：%s\n详情：%s\n\n堆栈：\n%s\n", time.Now().Format(time.RFC3339), kind, detail, stack)
	content = diagnostics.Redact(content)
	path := filepath.Join(dir, "crash-"+time.Now().UTC().Format("20060102-150405.000")+".log")
	if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
		return ""
	}
	return path
}

func (a *uiApp) run() error {
	instance, _, _ := procGetModuleHandleW.Call(0)
	className := utf16("DHNativeWindowV2")
	taskbarCreatedMessage, _, _ = user32.NewProc("RegisterWindowMessageW").Call(uintptr(unsafe.Pointer(utf16("TaskbarCreated"))))
	icon, _, _ := procLoadIconW.Call(instance, idiApp)
	if icon == 0 {
		icon, _, _ = procLoadIconW.Call(0, 32512)
	}
	a.icon = icon
	// The tray resource is generated from assets/tray.ico and has a white
	// backing so it remains visible against light and dark taskbars.
	// rsrc allocates manifest=1, window group=2 and tray group=9 when both
	// ICO files are embedded by the resources target.
	a.trayIcon, _, _ = procLoadIconW.Call(instance, uintptr(9))
	cursor, _, _ := procLoadCursorW.Call(0, idcArrow)
	wc := wndClassEx{
		Size: uint32(unsafe.Sizeof(wndClassEx{})), Style: 0,
		WndProc: syscall.NewCallback(wndProc), Instance: instance, Icon: icon, Cursor: cursor,
		Background: colorWindow + 1, ClassName: className, IconSmall: icon,
	}
	registered, _, registerErr := procRegisterClassExW.Call(uintptr(unsafe.Pointer(&wc)))
	if registered == 0 {
		return fmt.Errorf("register window class: %v", registerErr)
	}
	style := uintptr(wsOverlapped | wsCaption | wsSysMenu | wsThickFrame | wsMinimizeBox | wsMaximizeBox | wsVisible | wsClipChildren)
	hwnd, _, createErr := procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(className)), uintptr(unsafe.Pointer(utf16("DH BOT"))), style, 80, 60, uintptr(a.width), uintptr(a.height), 0, 0, instance, 0)
	if hwnd == 0 {
		return fmt.Errorf("create window: %v", createErr)
	}
	a.hwnd = hwnd
	a.addTrayIcon()
	a.createControls()
	a.layoutControls()
	a.refreshLists()
	a.launch("后台轮询", a.backgroundLoop)
	procShowWindow.Call(hwnd, swShow)
	procSetTimer.Call(hwnd, 1, 750, 0)
	procSetTimer.Call(hwnd, 2, 120, 0)
	procUpdateWindow.Call(hwnd)
	var message msg
	for {
		result, _, _ := procGetMessageW.Call(uintptr(unsafe.Pointer(&message)), 0, 0, 0)
		if int32(result) <= 0 {
			break
		}
		procTranslateMessage.Call(uintptr(unsafe.Pointer(&message)))
		procDispatchMessageW.Call(uintptr(unsafe.Pointer(&message)))
	}
	return nil
}

func wndProc(hwnd, message, wParam, lParam uintptr) uintptr {
	if activeAITestDialog != nil && activeAITestDialog.hwnd == hwnd {
		return aiTestWndProc(hwnd, message, wParam, lParam)
	}
	if app == nil {
		result, _, _ := procDefWindowProcW.Call(hwnd, message, wParam, lParam)
		return result
	}
	if taskbarCreatedMessage != 0 && message == taskbarCreatedMessage {
		app.removeTrayIcon()
		app.addTrayIcon()
		return 0
	}
	switch message {
	case wmCreate:
		return 0
	case wmGetMinMax:
		if info := (*minMaxInfo)(unsafe.Pointer(lParam)); info != nil {
			info.MinTrackSize = point{X: minWindowWidth, Y: minWindowHeight}
		}
		return 0
	case wmSize:
		if wParam == sizeMinimized {
			app.hideToTray()
			return 0
		}
		app.width = int32(uint16(lParam & 0xffff))
		app.height = int32(uint16((lParam >> 16) & 0xffff))
		app.layoutControls()
		app.invalidate()
		return 0
	case wmSysCommand:
		if wParam&0xFFF0 == scMinimize {
			app.hideToTray()
			return 0
		}
	case wmPaint:
		app.paint(hwnd)
		return 0
	case wmEraseBkgnd:
		return 1
	case wmTimer:
		if wParam == 2 {
			if app.advanceAnimation() {
				app.invalidate()
			}
			return 0
		}
		selectionChanged := app.syncSelectionsFromLists()
		app.refreshLists()
		if selectionChanged {
			app.invalidate()
		}
		return 0
	case wmLButtonUp:
		app.mouseDown = false
		app.click(int32(int16(lParam&0xffff)), int32(int16((lParam>>16)&0xffff)))
		return 0
	case wmLButtonDown:
		app.mouseDown = true
		app.invalidate()
		return 0
	case wmMouseMove:
		app.hover(int32(int16(lParam&0xffff)), int32(int16((lParam>>16)&0xffff)))
		return 0
	case wmMouseLeave:
		app.mouseInside = false
		app.mouseDown = false
		app.hoverText = ""
		app.invalidate()
		return 0
	case wmCommand:
		return 0
	case wmAppTray:
		switch lParam & 0xffff {
		case wmLButtonDbl:
			app.showFromTray()
		case wmRButtonUp, wmContextMenu:
			app.showTrayMenu()
		}
		return 0
	case wmAppTrayProbe:
		if app.trayAdded {
			return 1
		}
		return 0
	case wmAppShow:
		app.showFromTray()
		return 0
	case wmAppRefresh:
		app.refreshLists()
		app.populateMemberSuggestion()
		app.invalidate()
		return 0
	case wmClose:
		if app.exitRequested {
			procDestroyWindow.Call(hwnd)
		} else {
			switch app.setting("tray.close_behavior", "") {
			case "exit":
				app.exitRequested = true
				procDestroyWindow.Call(hwnd)
			case "tray":
				app.hideToTray()
			default:
				message := "关闭 DH BOT？\n\n选择“是”退出程序，选择“否”挂到托盘。下次沿用这个选择。"
				answer, _, _ := procMessageBoxW.Call(hwnd, uintptr(unsafe.Pointer(utf16(message))), uintptr(unsafe.Pointer(utf16("DH BOT"))), mbYesNo|mbIconWarning)
				if answer == idYes {
					_ = app.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: "tray.close_behavior", Value: "exit", UpdatedAt: time.Now().UTC()})
					app.exitRequested = true
					procDestroyWindow.Call(hwnd)
				} else {
					_ = app.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: "tray.close_behavior", Value: "tray", UpdatedAt: time.Now().UTC()})
					app.hideToTray()
				}
			}
		}
		return 0
	case wmDestroy:
		procKillTimer.Call(hwnd, 1)
		procKillTimer.Call(hwnd, 2)
		app.stopOnce.Do(func() { close(app.stop) })
		if app.bridgeService != nil {
			_ = app.bridgeService.Close()
		}
		app.waitWorkers(2 * time.Second)
		app.removeTrayIcon()
		_ = app.database.Close()
		app.releasePaintResources()
		app.releaseThemeResources()
		app.deleteFonts()
		procPostQuitMessage.Call(0)
		return 0
	}
	result, _, _ := procDefWindowProcW.Call(hwnd, message, wParam, lParam)
	return result
}

func (a *uiApp) createFonts() {
	a.fontBody = createFont(-16, 400)
	a.fontMedium = createFont(-16, 600)
	a.fontHeading = createFont(-24, 600)
	a.fontHero = createFont(-34, 600)
}

func createFont(height, weight int32) uintptr {
	font, _, _ := procCreateFontW.Call(uintptr(height), 0, 0, 0, uintptr(weight), 0, 0, 0, 1, 0, 0, 5, 0, uintptr(unsafe.Pointer(utf16("Microsoft YaHei UI"))))
	return font
}

func (a *uiApp) deleteFonts() {
	for _, font := range []uintptr{a.fontBody, a.fontMedium, a.fontHeading, a.fontHero} {
		if font != 0 {
			procDeleteObject.Call(font)
		}
	}
}

func (a *uiApp) waitWorkers(timeout time.Duration) {
	done := make(chan struct{})
	go func() {
		a.workerWG.Wait()
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(timeout):
	}
}

func (a *uiApp) themeBrush(color uint32) uintptr {
	if brush := a.brushes[color]; brush != 0 {
		return brush
	}
	brush, _, _ := procCreateSolidBrush.Call(uintptr(color))
	if brush != 0 {
		a.brushes[color] = brush
	}
	return brush
}

func (a *uiApp) themePen(color uint32) uintptr {
	if pen := a.pens[color]; pen != 0 {
		return pen
	}
	pen, _, _ := procCreatePen.Call(psSolid, 1, uintptr(color))
	if pen != 0 {
		a.pens[color] = pen
	}
	return pen
}

func (a *uiApp) releaseThemeResources() {
	for color, brush := range a.brushes {
		if brush != 0 {
			procDeleteObject.Call(brush)
		}
		delete(a.brushes, color)
	}
	for color, pen := range a.pens {
		if pen != 0 {
			procDeleteObject.Call(pen)
		}
		delete(a.pens, color)
	}
}

func (a *uiApp) releasePaintResources() {
	if a.backDC == 0 {
		return
	}
	if a.backOldBitmap != 0 {
		procSelectObject.Call(a.backDC, a.backOldBitmap)
	}
	if a.backBitmap != 0 {
		procDeleteObject.Call(a.backBitmap)
	}
	procDeleteDC.Call(a.backDC)
	a.backDC, a.backBitmap, a.backOldBitmap = 0, 0, 0
	a.backWidth, a.backHeight = 0, 0
}

func (a *uiApp) ensurePaintBuffer(hdc uintptr, width, height int32) uintptr {
	if a.backDC != 0 && a.backBitmap != 0 && a.backWidth == width && a.backHeight == height {
		return a.backDC
	}
	a.releasePaintResources()
	backDC, _, _ := procCreateCompatibleDC.Call(hdc)
	bitmap, _, _ := procCreateCompatibleBmp.Call(hdc, uintptr(max(width, 1)), uintptr(max(height, 1)))
	if backDC == 0 || bitmap == 0 {
		if bitmap != 0 {
			procDeleteObject.Call(bitmap)
		}
		if backDC != 0 {
			procDeleteDC.Call(backDC)
		}
		return 0
	}
	oldBitmap, _, _ := procSelectObject.Call(backDC, bitmap)
	a.backDC, a.backBitmap, a.backOldBitmap = backDC, bitmap, oldBitmap
	a.backWidth, a.backHeight = width, height
	return backDC
}

func (a *uiApp) addTrayIcon() {
	if a.hwnd == 0 || a.trayAdded {
		return
	}
	data := notifyIconData{
		Size: uint32(unsafe.Sizeof(notifyIconData{})), HWnd: a.hwnd, ID: 1,
		Flags: nifMessage | nifIcon | nifTip, CallbackMessage: wmAppTray, Icon: firstNonZero(a.trayIcon, a.icon),
	}
	copy(data.Tip[:], syscall.StringToUTF16("DH BOT 群管理"))
	result, _, _ := procShellNotifyIconW.Call(nimAdd, uintptr(unsafe.Pointer(&data)))
	a.trayAdded = result != 0
	if a.trayAdded {
		data.TimeoutOrVersion = 4
		procShellNotifyIconW.Call(nimSetVersion, uintptr(unsafe.Pointer(&data)))
	}
}

func firstNonZero(values ...uintptr) uintptr {
	for _, value := range values {
		if value != 0 {
			return value
		}
	}
	return 0
}

func (a *uiApp) removeTrayIcon() {
	if !a.trayAdded {
		return
	}
	data := notifyIconData{Size: uint32(unsafe.Sizeof(notifyIconData{})), HWnd: a.hwnd, ID: 1}
	procShellNotifyIconW.Call(nimDelete, uintptr(unsafe.Pointer(&data)))
	a.trayAdded = false
}

func (a *uiApp) hideToTray() {
	if a.hwnd == 0 {
		return
	}
	procShowWindow.Call(a.hwnd, swHide)
	a.trayHidden = true
}

func (a *uiApp) showFromTray() {
	if a.hwnd == 0 {
		return
	}
	procShowWindow.Call(a.hwnd, swRestore)
	procSetForegroundWindow.Call(a.hwnd)
	a.trayHidden = false
	a.layoutControls()
	a.invalidate()
}

func (a *uiApp) showTrayMenu() {
	menu, _, _ := procCreatePopupMenu.Call()
	if menu == 0 {
		return
	}
	defer procDestroyMenu.Call(menu)
	procAppendMenuW.Call(menu, 0, trayOpenCommand, uintptr(unsafe.Pointer(utf16("显示 DH BOT"))))
	paused := a.setting("automation.paused", "false") == "true"
	pauseLabel := onOff(paused, "恢复全部自动化", "暂停全部自动化")
	procAppendMenuW.Call(menu, 0, trayPauseCommand, uintptr(unsafe.Pointer(utf16(pauseLabel))))
	procAppendMenuW.Call(menu, mfSeparator, 0, 0)
	procAppendMenuW.Call(menu, 0, trayExitCommand, uintptr(unsafe.Pointer(utf16("退出 DH BOT"))))
	var cursor point
	procGetCursorPos.Call(uintptr(unsafe.Pointer(&cursor)))
	procSetForegroundWindow.Call(a.hwnd)
	command, _, _ := procTrackPopupMenu.Call(menu, tpmRightButton|tpmReturnCmd, uintptr(cursor.X), uintptr(cursor.Y), 0, a.hwnd, 0)
	procPostMessageW.Call(a.hwnd, wmNull, 0, 0)
	switch command {
	case trayOpenCommand:
		a.showFromTray()
	case trayPauseCommand:
		a.toggleGlobalPause()
	case trayExitCommand:
		a.exitRequested = true
		procDestroyWindow.Call(a.hwnd)
	}
}

type editSpec struct {
	id       int
	value    string
	multi    bool
	password bool
}

func (a *uiApp) createControls() {
	for _, spec := range []editSpec{
		{editWelcome, "欢迎 @「[成员]」加入群聊，请先查看群规。", false, false},
		{editMuteMinutes, "10", false, false},
		{editCardPrefix, "DH", false, false},
		{editCardSuggestion, "", false, false},
		{editMessageGroup, "", false, false}, {editMessageText, "", true, false},
		{editRuleGroup, "", false, false}, {editRulePattern, "", false, false}, {editRulePath, defaultUserPath("rules.json"), false, false},
		{editKnowledgeGroup, "", false, false}, {editKnowledgeTitle, "", false, false}, {editKnowledgePath, "", false, false}, {editKnowledgeContent, "", true, false}, {editKnowledgeBase, "", false, false}, {editKnowledgeDesc, "", true, false},
		{editTaskGroup, "", false, false}, {editTaskTitle, "", false, false}, {editTaskAssignee, "", false, false}, {editTaskDue, "", false, false},
		{editDevTools, a.setting("bridge.devtools", "http://127.0.0.1:9222"), false, false}, {editEndpoint, a.setting("bridge.endpoint", "http://127.0.0.1:51235"), false, false}, {editWangPath, a.setting("wsl.path", ""), false, false},
		{editScheduleName, a.setting("schedule.name", "每日群发言计划"), false, false},
		{editWakeWords, "@DH", false, false}, {editAIKind, a.setting("ai.kind", "openai"), false, false},
		{editAIBase, a.setting("ai.base_url", defaultAIBaseURL), false, false}, {editAIModel, a.setting("ai.model", defaultAIModel), false, false},
		{editAIKey, "", false, true}, {editWebhookURL, a.setting("ai.webhook_url", ""), false, false},
		{editSearch, "", false, false},
	} {
		style := uintptr(wsChild | wsBorder | wsTabStop | esAutoHScroll)
		if spec.multi {
			style = wsChild | wsBorder | wsTabStop | esMultiLine | esAutoVScroll | esWantReturn
		}
		if spec.password {
			style |= esPassword
		}
		control, _, _ := procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("EDIT"))), uintptr(unsafe.Pointer(utf16(spec.value))), style, 0, 0, 100, 30, a.hwnd, uintptr(spec.id), 0, 0)
		if control != 0 {
			a.edits[spec.id] = control
			procSendMessageW.Call(control, wmSetFont, a.fontBody, 1)
		}
	}
	for _, clock := range []struct {
		id    int
		value string
	}{{editScheduleOpen, a.setting("schedule.open", "08:00")}, {editScheduleClose, a.setting("schedule.close", "22:00")}} {
		control, _, _ := procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("SysDateTimePick32"))), 0, wsChild|wsTabStop|dtsTimeFormat, 0, 0, 100, 34, a.hwnd, uintptr(clock.id), 0, 0)
		if control == 0 {
			continue
		}
		a.edits[clock.id] = control
		procSendMessageW.Call(control, wmSetFont, a.fontBody, 1)
		procSendMessageW.Call(control, dtmSetFormatW, 0, uintptr(unsafe.Pointer(utf16("HH':'mm"))))
		a.setClockControl(control, clock.value)
	}
	a.summaryTime, _, _ = procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("SysDateTimePick32"))), 0, wsChild|wsTabStop|dtsTimeFormat|dtsShowNone, 0, 0, 180, 34, a.hwnd, uintptr(editSummarySchedule), 0, 0)
	if a.summaryTime != 0 {
		procSendMessageW.Call(a.summaryTime, wmSetFont, a.fontBody, 1)
		procSendMessageW.Call(a.summaryTime, dtmSetFormatW, 0, uintptr(unsafe.Pointer(utf16("HH':'mm"))))
		a.setSummarySchedule("")
	}
	for _, spec := range []struct {
		id      int
		label   string
		target  *uintptr
		checked bool
	}{{checkRuleRecall, "撤回", &a.ruleRecall, true}, {checkRuleMute, "禁言", &a.ruleMute, false}} {
		*spec.target, _, _ = procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("BUTTON"))), uintptr(unsafe.Pointer(utf16(spec.label))), wsChild|wsTabStop|bsAutoCheckbox, 0, 0, 96, 34, a.hwnd, uintptr(spec.id), 0, 0)
		if *spec.target != 0 {
			procSendMessageW.Call(*spec.target, wmSetFont, a.fontBody, 1)
			if spec.checked {
				procSendMessageW.Call(*spec.target, bmSetCheck, bstChecked, 0)
			}
		}
	}
	a.autoStart, _, _ = procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("BUTTON"))), uintptr(unsafe.Pointer(utf16("自动启动旺商聊"))), wsChild|wsTabStop|bsAutoCheckbox, 0, 0, 180, 34, a.hwnd, uintptr(checkAutoStart), 0, 0)
	if a.autoStart != 0 {
		procSendMessageW.Call(a.autoStart, wmSetFont, a.fontBody, 1)
		if a.setting("wsl.auto_start", "true") != "false" {
			procSendMessageW.Call(a.autoStart, bmSetCheck, bstChecked, 0)
		}
	}
	a.persistLogin, _, _ = procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("BUTTON"))), uintptr(unsafe.Pointer(utf16("保留旺商聊登录状态"))), wsChild|wsTabStop|bsAutoCheckbox, 0, 0, 200, 34, a.hwnd, uintptr(checkPersistLogin), 0, 0)
	if a.persistLogin != 0 {
		procSendMessageW.Call(a.persistLogin, wmSetFont, a.fontBody, 1)
		if a.setting("wsl.persist_login", "true") != "false" {
			procSendMessageW.Call(a.persistLogin, bmSetCheck, bstChecked, 0)
		}
	}
	if a.scheduleEnabled, _, _ = procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("BUTTON"))), uintptr(unsafe.Pointer(utf16("启用计划"))), wsChild|wsTabStop|bsAutoCheckbox, 0, 0, 120, 34, a.hwnd, uintptr(checkSchedule), 0, 0); a.scheduleEnabled != 0 {
		procSendMessageW.Call(a.scheduleEnabled, wmSetFont, a.fontBody, 1)
		if a.setting("schedule.enabled", "false") == "true" {
			procSendMessageW.Call(a.scheduleEnabled, bmSetCheck, bstChecked, 0)
		}
	}
	if values, _ := a.secretStore.Load(); values["ai.default.apiKey"] != "" {
		a.apiKeyStored = true
		a.setEdit(editAIKey, secretPlaceholder)
	}
	a.createListControls()
}

func (a *uiApp) createListControls() {
	specs := []struct {
		id      int
		columns []string
		widths  []int32
	}{
		{listOverview, []string{"时间", "群", "内部 ID", "动作", "状态", "原因"}, []int32{120, 180, 110, 120, 90, 300}},
		{listSummaries, []string{"时间", "群", "来源", "摘要"}, []int32{120, 190, 90, 600}},
		{listGroups, []string{"群名", "群 ID", "状态", "AI", "规则", "任务", "AI 模式"}, []int32{240, 140, 90, 80, 80, 80, 140}},
		{listMembers, []string{"序号", "当前名称", "原名称", "建议 / 目标", "后缀", "来源", "状态", "内部身份"}, []int32{70, 150, 150, 160, 72, 88, 100, 130}},
		{listMessages, []string{"时间", "群", "成员", "类型", "内容"}, []int32{110, 180, 170, 90, 400}},
		{listRules, []string{"群", "规则", "匹配", "条件", "模式", "状态", "动作"}, []int32{130, 180, 100, 150, 90, 90, 220}},
		{listKnowledge, []string{"知识库", "标题", "类型", "使用群", "状态", "来源", "内容"}, []int32{190, 200, 90, 170, 90, 220, 300}},
		{listTasks, []string{"群", "状态", "任务", "负责人", "截止时间"}, []int32{170, 100, 300, 150, 170}},
		{listSchedules, []string{"计划", "状态", "开群", "关群", "群组", "下一动作"}, []int32{190, 90, 90, 90, 350, 180}},
		{listAudit, []string{"时间", "级别", "群", "成员", "事件", "详情"}, []int32{120, 80, 110, 110, 150, 360}},
	}
	for _, spec := range specs {
		style := uintptr(wsChild | wsTabStop | lvsReport | lvsSingleSel | lvsShowSelAlways)
		if spec.id == listMembers {
			style &^= lvsSingleSel
		}
		control, _, _ := procCreateWindowExW.Call(wsExClientEdge, uintptr(unsafe.Pointer(utf16("SysListView32"))), 0, style, 0, 0, 100, 100, a.hwnd, uintptr(spec.id), 0, 0)
		if control == 0 {
			continue
		}
		a.lists[spec.id] = control
		procSendMessageW.Call(control, wmSetFont, a.fontBody, 1)
		extended := uintptr(lvsExFullRowSelect | lvsExDoubleBuffer)
		if spec.id == listMembers {
			extended |= lvsExCheckboxes
		}
		procSendMessageW.Call(control, lvmSetExtendedStyle, 0, extended)
		for index, title := range spec.columns {
			column := listViewColumn{Mask: lvcfFmt | lvcfWidth | lvcfText | lvcfSubItem, Width: spec.widths[index], Text: utf16(title), SubItem: int32(index)}
			procSendMessageW.Call(control, lvmInsertColumnW, uintptr(index), uintptr(unsafe.Pointer(&column)))
		}
	}
}

func (a *uiApp) layoutControls() {
	visibilityChanged := !a.layoutReady || a.layoutPage != a.page || a.layoutGroupTab != a.groupTab
	if visibilityChanged {
		for _, control := range a.edits {
			procShowWindow.Call(control, swHide)
		}
		for _, control := range a.lists {
			procShowWindow.Call(control, swHide)
		}
		if a.summaryTime != 0 {
			procShowWindow.Call(a.summaryTime, swHide)
		}
		for _, control := range []uintptr{a.ruleRecall, a.ruleMute} {
			if control != 0 {
				procShowWindow.Call(control, swHide)
			}
		}
		if a.autoStart != 0 {
			procShowWindow.Call(a.autoStart, swHide)
		}
		if a.persistLogin != 0 {
			procShowWindow.Call(a.persistLogin, swHide)
		}
		if a.scheduleEnabled != 0 {
			procShowWindow.Call(a.scheduleEnabled, swHide)
		}
	}
	if a.hwnd == 0 || a.width < 760 {
		return
	}
	a.layoutPage, a.layoutGroupTab, a.layoutReady = a.page, a.groupTab, true
	x, content := int32(248), a.width-int32(280)
	if a.page != pageSettings && a.page != pageDebug {
		header := a.headerLayout()
		a.moveEdit(editSearch, header.Search.Left, header.Search.Top, header.Search.Right-header.Search.Left, header.Search.Bottom-header.Search.Top)
	}
	switch a.page {
	case pageOverview:
		a.moveList(listSummaries, x, 310, content, 170)
		a.moveList(listOverview, x, 530, content, max(a.height-554, 160))
	case pageGroups:
		groupUI := a.groupLayout()
		a.moveEdit(editWelcome, x+24, 130, content-278, 34)
		if a.summaryTime != 0 {
			procSetWindowPos.Call(a.summaryTime, 0, uintptr(x+content-234), 130, 210, 34, 0x0154)
		}
		if a.groupTab == 0 {
			a.moveList(listGroups, x, groupUI.GroupListTop, content, max(a.height-groupUI.GroupListTop-24, 180))
		} else {
			listHeight := groupUI.MemberPrimaryTop - groupUI.MemberListTop - 38
			a.moveList(listMembers, x, groupUI.MemberListTop, content, max(listHeight, 160))
			a.moveEdit(editMuteMinutes, x+118, groupUI.MemberPrimaryTop, 76, 36)
			a.moveEdit(editCardSuggestion, x+318, groupUI.MemberPrimaryTop, 104, 36)
		}
	case pageMessages:
		a.moveEdit(editMessageText, x+24, 206, content-48, 104)
		a.moveList(listMessages, x, 438, content, max(a.height-462, 180))
	case pageRules:
		addLeft := a.width - 154
		muteX, recallX := addLeft-100, addLeft-200
		a.moveEdit(editRulePattern, x+304, 130, max(recallX-(x+304)-20, 180), 34)
		procSetWindowPos.Call(a.ruleRecall, 0, uintptr(recallX), 130, 88, 34, 0x0154)
		procSetWindowPos.Call(a.ruleMute, 0, uintptr(muteX), 130, 88, 34, 0x0154)
		a.moveEdit(editRulePath, x+24, 206, content-270, 34)
		a.moveList(listRules, x, 280, content, max(a.height-304, 220))
	case pageKnowledge:
		a.moveEdit(editKnowledgeTitle, x+304, 130, 260, 34)
		a.moveEdit(editKnowledgePath, x+584, 130, content-608, 34)
		a.moveEdit(editKnowledgeBase, x+24, 250, 260, 34)
		a.moveEdit(editKnowledgeDesc, x+304, 250, content-328, 34)
		a.moveEdit(editKnowledgeContent, x+24, 322, content-48, 96)
		a.moveList(listKnowledge, x, 484, content, max(a.height-508, 160))
	case pageTasks:
		right := a.width - 24
		dueX := right - 180
		assigneeX := dueX - 150
		a.moveEdit(editTaskTitle, x+304, 130, max(assigneeX-(x+304)-20, 220), 34)
		a.moveEdit(editTaskAssignee, assigneeX, 130, 130, 34)
		a.moveEdit(editTaskDue, dueX, 130, 180, 34)
		a.moveEdit(editScheduleName, x+24, 254, 260, 34)
		a.moveEdit(editScheduleOpen, x+304, 254, 100, 34)
		a.moveEdit(editScheduleClose, x+424, 254, 100, 34)
		if a.scheduleEnabled != 0 {
			procSetWindowPos.Call(a.scheduleEnabled, 0, uintptr(x+544), 254, 120, 34, 0x0154)
			procShowWindow.Call(a.scheduleEnabled, swShow)
		}
		a.moveList(listSchedules, x, 304, content, 100)
		a.moveList(listTasks, x, 424, content, max(a.height-524, 180))
	case pageAudit:
		a.moveList(listAudit, x, 184, content, max(a.height-208, 280))
	case pageSettings:
		a.moveEdit(editWangPath, x+24, 130, 650, 34)
		if a.autoStart != 0 {
			procSetWindowPos.Call(a.autoStart, 0, uintptr(x+694), 130, 180, 34, 0x0154)
			procShowWindow.Call(a.autoStart, swShow)
		}
		if a.persistLogin != 0 {
			procSetWindowPos.Call(a.persistLogin, 0, uintptr(x+694), 166, 200, 34, 0x0154)
			procShowWindow.Call(a.persistLogin, swShow)
		}
		a.moveEdit(editDevTools, x+24, 230, 410, 34)
		a.moveEdit(editEndpoint, x+454, 230, 410, 34)
		a.moveEdit(editWakeWords, x+24, 310, 250, 34)
		a.moveEdit(editAIKind, x+294, 310, 160, 34)
		a.moveEdit(editAIModel, x+474, 310, 390, 34)
		a.moveEdit(editAIBase, x+24, 390, 410, 34)
		a.moveEdit(editWebhookURL, x+454, 390, 410, 34)
		a.moveEdit(editAIKey, x+24, 470, 840, 34)
		a.moveEdit(editCardPrefix, x+24, 608, 180, 34)
	}
}

func (a *uiApp) moveList(id int, x, y, width, height int32) {
	if control := a.lists[id]; control != 0 {
		procSetWindowPos.Call(control, 0, uintptr(x), uintptr(y), uintptr(max(width, 80)), uintptr(max(height, 80)), 0x0154)
	}
}

func (a *uiApp) refreshLists() {
	groupNames := a.groupNames()
	switch a.page {
	case pageOverview:
		summaries, _ := a.database.ListDailySummaries(context.Background(), a.accountID, 100)
		summaryRows := make([]listRow, 0, len(summaries))
		for _, summary := range summaries {
			source := onOff(summary.Source == "scheduled", "定时", "手动")
			summaryRows = append(summaryRows, listRow{ID: summary.ID, Values: []string{clock(summary.CreatedAt), firstNonEmpty(groupNames[summary.GroupID], "未知群"), source, summary.Content}})
		}
		a.populateList(listSummaries, summaryRows, a.selectedSummary)
		actions, _ := a.database.ListRecentActions(context.Background(), 0, 100)
		rows := make([]listRow, 0, len(actions))
		for _, action := range actions {
			state := "成功"
			if !action.Success {
				state = "失败"
			}
			rows = append(rows, listRow{ID: action.ID, Values: []string{clock(action.CreatedAt), firstNonEmpty(groupNames[action.GroupID], "未知群"), idText(action.UserID), actionText(action.Type), state, firstNonEmpty(action.Error, action.Reason)}})
		}
		a.populateList(listOverview, a.pagedRows(listOverview, rows), 0)
	case pageGroups:
		groups := a.groups()
		rows := make([]listRow, 0, len(groups))
		for _, group := range groups {
			rows = append(rows, listRow{ID: group.GroupID, Values: []string{firstNonEmpty(group.Name, "未命名群"), idText(group.GroupID), boolText(group.Enabled), boolText(group.AIEnabled), boolText(group.ModerationEnabled), boolText(group.TaskReminders), onOff(group.ManualTakeover, "人工接管", "AI 自动回复")}})
		}
		a.populateList(listGroups, a.pagedRowsQuery(listGroups, rows, ""), a.selectedGroup)
		members, _ := a.database.ListMembers(context.Background(), a.accountID, a.selectedGroup)
		preview := a.cardPreviewSnapshot(a.selectedGroup)
		sort.SliceStable(members, func(i, j int) bool {
			left, right := memberDisplayName(members[i]), memberDisplayName(members[j])
			leftMissing, rightMissing := left == "名称未设置", right == "名称未设置"
			if leftMissing != rightMissing {
				return !leftMissing
			}
			if left != right {
				return left < right
			}
			return members[i].UserID < members[j].UserID
		})
		rows = rows[:0]
		for index, member := range members {
			suggested := firstNonEmpty(member.ManagedCardName, member.CardName)
			if plan, exists := preview[member.UserID]; exists {
				suggested = plan.SuggestedName
			}
			rows = append(rows, listRow{ID: member.UserID, Values: []string{
				strconv.Itoa(index + 1), memberDisplayName(member), firstNonEmpty(member.OriginalCardName, "名称未设置"), firstNonEmpty(suggested, "待生成"),
				firstNonEmpty(member.CardSuffix, "-"), cardJoinSourceText(member.JoinSource), cardStatusText(member.CardStatus), memberIdentityText(member),
			}})
		}
		a.populateList(listMembers, a.pagedRowsQuery(listMembers, rows, ""), a.selectedMember)
	case pageMessages:
		messages, _ := a.database.ListRecentMessages(context.Background(), a.accountID, 0, 500)
		selected := a.selectedGroupSet(pageMessages)
		rows := make([]listRow, 0, len(messages))
		for _, message := range messages {
			if !selected[message.GroupID] {
				continue
			}
			rows = append(rows, listRow{ID: message.ID, Values: []string{clock(message.SentAt), firstNonEmpty(groupNames[message.GroupID], "未知群"), firstNonEmpty(message.SenderName, "名称未同步"), messageKindText(message.Kind), message.Text}})
		}
		a.populateList(listMessages, a.pagedRows(listMessages, rows), 0)
	case pageRules:
		rows := make([]listRow, 0)
		seen := make(map[int64]bool)
		groupIDs := a.selectedGroupIDs(pageRules)
		if len(groupIDs) == 0 {
			groupIDs = []int64{0}
		}
		for _, groupID := range groupIDs {
			rules, _ := a.database.ListRules(context.Background(), groupID)
			for _, rule := range rules {
				if seen[rule.ID] {
					continue
				}
				seen[rule.ID] = true
				actions := make([]string, 0, len(rule.Actions))
				for _, action := range rule.Actions {
					actions = append(actions, actionText(action.Type))
				}
				condition := rule.Pattern
				if condition == "" {
					condition = fmt.Sprintf("阈值 %d / 次数 %d", rule.Threshold, rule.Count)
				}
				scope := firstNonEmpty(groupNames[rule.GroupID], "未知群")
				if rule.GroupID == 0 {
					scope = "全局规则"
				}
				rows = append(rows, listRow{ID: rule.ID, Values: []string{scope, rule.Name, matcherText(rule.Matcher), condition, ruleModeText(rule.Mode), ruleEnabledText(rule.Enabled), strings.Join(actions, "+")}})
			}
		}
		a.populateList(listRules, a.pagedRows(listRules, rows), a.selectedRule)
	case pageKnowledge:
		rows := make([]listRow, 0)
		a.refreshKnowledgeBases()
		if a.selectedKnowledgeBase != 0 {
			knowledgeAccount := a.knowledgeAccountID()
			items, _ := a.database.ListKnowledgeBasesDocuments(context.Background(), knowledgeAccount, a.selectedKnowledgeBase)
			bindings, _ := a.database.ListKnowledgeBindings(context.Background(), knowledgeAccount, a.selectedKnowledgeBase)
			boundNames := make([]string, 0, len(bindings))
			for _, binding := range bindings {
				if binding.Enabled {
					boundNames = append(boundNames, firstNonEmpty(groupNames[binding.GroupID], "未知群"))
				}
			}
			usage := strings.Join(boundNames, "、")
			if usage == "" {
				usage = "未绑定群，AI 不使用"
			}
			for _, item := range items {
				baseName := item.BaseName
				status := "已启用"
				for _, base := range a.knowledgeBases {
					if base.ID == item.BaseID {
						if !base.Enabled {
							status = "已停用"
						}
						baseName = base.Name
					}
				}
				rows = append(rows, listRow{ID: item.ID, Values: []string{baseName, item.Title, knowledgeKindText(item.Kind), usage, status, item.Source, item.Content}})
			}
		}
		a.populateList(listKnowledge, a.pagedRows(listKnowledge, rows), 0)
	case pageTasks:
		rows := make([]listRow, 0)
		for _, groupID := range a.selectedGroupIDs(pageTasks) {
			tasks, _ := a.database.ListTasks(context.Background(), groupID, "")
			for _, task := range tasks {
				due := "-"
				if task.DueAt != nil {
					due = task.DueAt.Local().Format("01-02 15:04")
				}
				rows = append(rows, listRow{ID: task.ID, Values: []string{groupNames[groupID], taskStatusText(task.Status), task.Title, idText(task.AssigneeID), due}})
			}
		}
		a.populateList(listTasks, a.pagedRows(listTasks, rows), a.selectedTask)
		scheduleRows := make([]listRow, 0)
		for _, schedule := range a.groupSchedules() {
			status := "已停用"
			if schedule.Enabled {
				status = "已启用"
			}
			bound := make([]string, 0, len(schedule.GroupIDs))
			for _, groupID := range schedule.GroupIDs {
				bound = append(bound, firstNonEmpty(groupNames[groupID], "未知群"))
			}
			scheduleRows = append(scheduleRows, listRow{ID: schedule.ID, Values: []string{schedule.Name, status, schedule.OpenTime, schedule.CloseTime, strings.Join(bound, "、"), scheduleNextAction(schedule)}})
		}
		a.populateList(listSchedules, scheduleRows, 0)
	case pageAudit:
		audits, _ := a.database.ListAudit(context.Background(), 0, 1000)
		selected := a.selectedGroupSet(pageAudit)
		rows := make([]listRow, 0, len(audits))
		for _, audit := range audits {
			if !selected[audit.GroupID] {
				continue
			}
			rows = append(rows, listRow{ID: audit.ID, Values: []string{clock(audit.CreatedAt), auditLevelText(audit.Level), firstNonEmpty(groupNames[audit.GroupID], "未知群"), idText(audit.UserID), auditEventText(audit.Event), localizeUserText(audit.Details)}})
		}
		a.populateList(listAudit, a.pagedRows(listAudit, rows), 0)
	}
}

func (a *uiApp) cardPreviewSnapshot(groupID int64) map[int64]groupmgr.CardPlan {
	a.mu.RLock()
	defer a.mu.RUnlock()
	if a.cardPreviewGroup != groupID {
		return nil
	}
	out := make(map[int64]groupmgr.CardPlan, len(a.cardPreview))
	for userID, item := range a.cardPreview {
		out[userID] = item
	}
	return out
}

func (a *uiApp) populateList(id int, rows []listRow, selectedID int64) {
	control := a.lists[id]
	if control == 0 {
		return
	}
	var signature strings.Builder
	for _, row := range rows {
		signature.WriteByte('|')
		signature.WriteString(strconv.FormatInt(row.ID, 10))
		for _, value := range row.Values {
			signature.WriteByte('\x00')
			signature.WriteString(value)
		}
	}
	if value := signature.String(); a.listSignatures[id] == value {
		return
	} else {
		a.listSignatures[id] = value
	}
	procSendMessageW.Call(control, wmSetRedraw, 0, 0)
	defer func() {
		procSendMessageW.Call(control, wmSetRedraw, 1, 0)
		procInvalidateRect.Call(control, 0, 0)
	}()
	procSendMessageW.Call(control, lvmDeleteAllItems, 0, 0)
	for rowIndex, row := range rows {
		first := ""
		if len(row.Values) > 0 {
			first = row.Values[0]
		}
		item := listViewItem{Mask: lvifText | lvifParam, Item: int32(rowIndex), Text: utf16(first), Param: uintptr(row.ID)}
		procSendMessageW.Call(control, lvmInsertItemW, 0, uintptr(unsafe.Pointer(&item)))
		for columnIndex := 1; columnIndex < len(row.Values); columnIndex++ {
			cell := listViewItem{Mask: lvifText, Item: int32(rowIndex), SubItem: int32(columnIndex), Text: utf16(row.Values[columnIndex])}
			procSendMessageW.Call(control, lvmSetItemW, 0, uintptr(unsafe.Pointer(&cell)))
		}
		if selectedID != 0 && row.ID == selectedID {
			selection := listViewItem{State: lvisSelected | lvisFocused, StateMask: lvisSelected | lvisFocused}
			procSendMessageW.Call(control, lvmSetItemState, uintptr(rowIndex), uintptr(unsafe.Pointer(&selection)))
		}
		if id == listMembers {
			state := uint32(1 << 12)
			if a.selectedMembers[row.ID] {
				state = 2 << 12
			}
			checkbox := listViewItem{State: state, StateMask: lvisStateImageMask}
			procSendMessageW.Call(control, lvmSetItemState, uintptr(rowIndex), uintptr(unsafe.Pointer(&checkbox)))
		}
	}
}

func (a *uiApp) syncSelectionsFromLists() bool {
	changed := false
	switch a.page {
	case pageOverview:
		if summaryID := a.selectedListID(listSummaries); summaryID > 0 && summaryID != a.selectedSummary {
			a.selectedSummary = summaryID
			changed = true
		}
	case pageGroups:
		if a.groupTab == 0 {
			if groupID := a.selectedListID(listGroups); groupID > 0 && groupID != a.selectedGroup {
				a.selectedGroup, a.selectedMember = groupID, 0
				clear(a.selectedMembers)
				a.populateGroupEdits()
				changed = true
			}
		} else {
			if a.syncCheckedMembers() {
				changed = true
			}
			if userID := a.selectedListID(listMembers); userID != 0 && userID != a.selectedMember {
				a.selectedMember = userID
				a.populateMemberSuggestion()
				changed = true
			}
		}
	case pageTasks:
		if taskID := a.selectedListID(listTasks); taskID > 0 && taskID != a.selectedTask {
			a.selectedTask = taskID
			changed = true
		}
	case pageRules:
		if ruleID := a.selectedListID(listRules); ruleID > 0 && ruleID != a.selectedRule {
			a.selectedRule = ruleID
			changed = true
		}
	}
	return changed
}

func (a *uiApp) syncCheckedMembers() bool {
	control := a.lists[listMembers]
	if control == 0 {
		return false
	}
	count, _, _ := procSendMessageW.Call(control, lvmGetItemCount, 0, 0)
	changed := false
	for index := 0; index < int(count); index++ {
		item := listViewItem{Mask: lvifParam, Item: int32(index)}
		if result, _, _ := procSendMessageW.Call(control, lvmGetItemW, 0, uintptr(unsafe.Pointer(&item))); result == 0 {
			continue
		}
		state, _, _ := procSendMessageW.Call(control, lvmGetItemState, uintptr(index), lvisStateImageMask)
		checked := (state>>12)&0xF == 2
		memberID := int64(item.Param)
		if a.selectedMembers[memberID] != checked {
			changed = true
			if checked {
				a.selectedMembers[memberID] = true
			} else {
				delete(a.selectedMembers, memberID)
			}
		}
	}
	return changed
}

func (a *uiApp) checkedMemberIDs() []int64 {
	ids := make([]int64, 0, len(a.selectedMembers))
	for userID, selected := range a.selectedMembers {
		if selected {
			ids = append(ids, userID)
		}
	}
	sort.Slice(ids, func(i, j int) bool { return ids[i] < ids[j] })
	return ids
}

func (a *uiApp) selectedListIndex(id int) int {
	control := a.lists[id]
	if control == 0 {
		return -1
	}
	result, _, _ := procSendMessageW.Call(control, lvmGetNextItem, ^uintptr(0), lvniSelected)
	return int(int32(result))
}

func (a *uiApp) selectedListID(id int) int64 {
	control := a.lists[id]
	index := a.selectedListIndex(id)
	if control == 0 || index < 0 {
		return 0
	}
	item := listViewItem{Mask: lvifParam, Item: int32(index)}
	result, _, _ := procSendMessageW.Call(control, lvmGetItemW, 0, uintptr(unsafe.Pointer(&item)))
	if result == 0 {
		return 0
	}
	return int64(item.Param)
}

func (a *uiApp) pagedRows(id int, rows []listRow) []listRow {
	return a.pagedRowsQuery(id, rows, strings.ToLower(strings.TrimSpace(a.getEdit(editSearch))))
}

func (a *uiApp) pagedRowsQuery(id int, rows []listRow, query string) []listRow {
	if previous := a.searches[id]; previous != query {
		a.searches[id] = query
		a.listPages[id] = 0
	}
	filtered := rows
	if query != "" {
		filtered = make([]listRow, 0, len(rows))
		for _, row := range rows {
			if strings.Contains(strings.ToLower(strings.Join(row.Values, "\t")), query) {
				filtered = append(filtered, row)
			}
		}
	}
	pageSize := listPageSize(id)
	a.listTotals[id] = len(filtered)
	maxPage := 0
	if len(filtered) > 0 {
		maxPage = (len(filtered) - 1) / pageSize
	}
	page := a.listPages[id]
	if page > maxPage {
		page = maxPage
		a.listPages[id] = page
	}
	start := page * pageSize
	end := min(start+pageSize, len(filtered))
	return filtered[start:end]
}

func (a *uiApp) currentListID() int {
	switch a.page {
	case pageOverview:
		return listOverview
	case pageGroups:
		if a.groupTab == 0 {
			return listGroups
		}
		return listMembers
	case pageMessages:
		return listMessages
	case pageRules:
		return listRules
	case pageKnowledge:
		return listKnowledge
	case pageTasks:
		return listTasks
	case pageAudit:
		return listAudit
	default:
		return 0
	}
}

func (a *uiApp) switchGroupTab(tab int) {
	if tab < 0 || tab > 1 || tab == a.groupTab {
		return
	}
	oldList := a.currentListID()
	if oldList != 0 {
		a.searches[oldList] = strings.TrimSpace(a.getEdit(editSearch))
	}
	a.groupTab = tab
	newList := a.currentListID()
	if newList != 0 {
		a.setEdit(editSearch, a.searches[newList])
	}
	a.layoutControls()
	a.refreshLists()
	a.invalidate()
	if tab == 1 && a.selectedGroup != 0 {
		a.previewCards()
	}
}

func (a *uiApp) pageLabel() string {
	id := a.currentListID()
	if id == 0 {
		return ""
	}
	total := a.listTotals[id]
	pageSize := listPageSize(id)
	pages := 1
	if total > 0 {
		pages = (total + pageSize - 1) / pageSize
	}
	unit := "条"
	if id == listMembers {
		unit = "人"
	}
	return fmt.Sprintf("第 %d / %d 页 · 共 %d %s", a.listPages[id]+1, pages, total, unit)
}

type headerRects struct {
	Title, Search, Previous, Next, Page, Status rect
}

type groupRects struct {
	Sync, Enabled, AI, Moderation, Manual, Tasks, Save, GroupMute, GroupUnmute rect
	GroupTab, MemberTab, Auto, Preview, Execute, Pause, Retry, Notices         rect
	MemberMute, MemberUnmute, MemberApply, Cleanup, Restore, Remove            rect
	GroupListTop, MemberListTop, MemberPrimaryTop, MemberSecondaryTop          int32
	Compact                                                                    bool
}

func (a *uiApp) headerLayout() headerRects {
	status := rect{a.width - 180, 20, a.width - 24, 58}
	page := rect{status.Left - 200, 24, status.Left - 10, 56}
	next := rect{page.Left - 76, 24, page.Left - 8, 56}
	previous := rect{next.Left - 76, 24, next.Left - 8, 56}
	search := rect{previous.Left - 192, 24, previous.Left - 12, 56}
	titleRight := search.Left - 16
	if a.page == pageSettings || a.page == pageDebug {
		titleRight = status.Left - 16
	}
	return headerRects{
		Title: rect{248, 20, max(titleRight, 420), 58}, Search: search,
		Previous: previous, Next: next, Page: page, Status: status,
	}
}

func (a *uiApp) groupLayout() groupRects {
	x := int32(248)
	primaryTop := max(a.height-82, 690)
	layout := groupRects{
		Sync: rect{x + 24, 178, x + 110, 214}, Enabled: rect{x + 120, 178, x + 224, 214},
		AI: rect{x + 234, 178, x + 320, 214}, Moderation: rect{x + 330, 178, x + 426, 214},
		Manual: rect{x + 436, 178, x + 540, 214}, Tasks: rect{x + 550, 178, x + 668, 214},
		Save: rect{x + 678, 178, x + 790, 214}, GroupMute: rect{x + 800, 178, x + 886, 214}, GroupUnmute: rect{x + 896, 178, x + 988, 214},
		GroupTab: rect{x, 226, x + 132, 262}, MemberTab: rect{x + 140, 226, x + 272, 262},
		Auto: rect{x + 24, 272, x + 122, 308}, Preview: rect{x + 130, 272, x + 230, 308}, Execute: rect{x + 238, 272, x + 342, 308},
		Pause: rect{x + 350, 272, x + 438, 308}, Retry: rect{x + 446, 272, x + 546, 308}, Notices: rect{x + 554, 272, x + 626, 308},
		MemberMute: rect{x + 24, primaryTop, x + 106, primaryTop + 36}, MemberUnmute: rect{x + 202, primaryTop, x + 286, primaryTop + 36},
		MemberApply: rect{x + 430, primaryTop, x + 532, primaryTop + 36}, Cleanup: rect{x + 544, primaryTop, x + 674, primaryTop + 36},
		Restore: rect{a.width - 330, primaryTop, a.width - 136, primaryTop + 36}, Remove: rect{a.width - 124, primaryTop, a.width - 24, primaryTop + 36},
		GroupListTop: 274, MemberListTop: 342, MemberPrimaryTop: primaryTop, MemberSecondaryTop: primaryTop,
	}
	if a.width >= 1240 {
		return layout
	}
	layout.Compact = true
	layout.Save = rect{x + 24, 222, x + 136, 258}
	layout.GroupMute = rect{x + 146, 222, x + 232, 258}
	layout.GroupUnmute = rect{x + 242, 222, x + 334, 258}
	layout.GroupTab = rect{x, 268, x + 132, 304}
	layout.MemberTab = rect{x + 140, 268, x + 272, 304}
	layout.Auto = rect{x + 24, 314, x + 122, 350}
	layout.Preview = rect{x + 130, 314, x + 230, 350}
	layout.Execute = rect{x + 238, 314, x + 342, 350}
	layout.Pause = rect{x + 350, 314, x + 438, 350}
	layout.Retry = rect{x + 446, 314, x + 546, 350}
	layout.Notices = rect{x + 554, 314, x + 626, 350}
	primaryTop = max(a.height-104, 612)
	secondaryTop := max(a.height-58, 658)
	layout.MemberMute = rect{x + 24, primaryTop, x + 106, primaryTop + 36}
	layout.MemberUnmute = rect{x + 202, primaryTop, x + 286, primaryTop + 36}
	layout.MemberApply = rect{x + 430, primaryTop, x + 532, primaryTop + 36}
	layout.Cleanup = rect{x + 24, secondaryTop, x + 154, secondaryTop + 36}
	layout.Restore = rect{x + 166, secondaryTop, x + 360, secondaryTop + 36}
	layout.Remove = rect{x + 372, secondaryTop, x + 472, secondaryTop + 36}
	layout.GroupListTop, layout.MemberListTop = 316, 384
	layout.MemberPrimaryTop, layout.MemberSecondaryTop = primaryTop, secondaryTop
	return layout
}

func listPageSize(id int) int {
	if id == listMembers {
		return 200
	}
	return 50
}

func (a *uiApp) bottomActionTop() int32 {
	return max(a.height-58, 650)
}

func (a *uiApp) moveEdit(id int, x, y, width, height int32) {
	if control := a.edits[id]; control != 0 {
		procSetWindowPos.Call(control, 0, uintptr(x), uintptr(y), uintptr(max(width, 40)), uintptr(height), 0x0154)
	}
}

func (a *uiApp) paint(hwnd uintptr) {
	var ps paintStruct
	hdc, _, _ := procBeginPaint.Call(hwnd, uintptr(unsafe.Pointer(&ps)))
	if hdc == 0 {
		return
	}
	defer procEndPaint.Call(hwnd, uintptr(unsafe.Pointer(&ps)))
	var client rect
	procGetClientRect.Call(hwnd, uintptr(unsafe.Pointer(&client)))
	a.width, a.height = client.Right, client.Bottom
	drawDC := hdc
	memoryDC := a.ensurePaintBuffer(hdc, client.Right, client.Bottom)
	if memoryDC != 0 {
		drawDC = memoryDC
	}
	fill(drawDC, client, rgb(255, 255, 255))
	a.paintSidebar(drawDC)
	a.paintHeader(drawDC)
	switch a.page {
	case pageOverview:
		a.paintOverview(drawDC)
	case pageGroups:
		a.paintGroups(drawDC)
	case pageMessages:
		a.paintMessages(drawDC)
	case pageRules:
		a.paintRules(drawDC)
	case pageKnowledge:
		a.paintKnowledge(drawDC)
	case pageTasks:
		a.paintTasks(drawDC)
	case pageAudit:
		a.paintAudit(drawDC)
	case pageSettings:
		a.paintSettings(drawDC)
	case pageDebug:
		a.paintDebug(drawDC)
	}
	a.paintTooltip(drawDC)
	if drawDC == memoryDC && memoryDC != 0 {
		procBitBlt.Call(hdc, 0, 0, uintptr(client.Right), uintptr(client.Bottom), memoryDC, 0, 0, 0x00CC0020)
	}
}

func (a *uiApp) paintSidebar(hdc uintptr) {
	fill(hdc, rect{0, 0, 216, a.height}, rgb(250, 250, 250))
	fill(hdc, rect{215, 0, 216, a.height}, rgb(232, 232, 232))
	if a.icon != 0 {
		procDrawIconEx.Call(hdc, 24, 23, a.icon, 34, 34, 0, 0, 3)
	}
	drawText(hdc, rect{68, 20, 184, 55}, "DH BOT", rgb(10, 10, 10), a.fontHeading, dtLeft|dtVCenter|dtSingleLine)
	for index, label := range navLabels {
		y := sidebarNavTop + int32(index*44)
		if index == a.page {
			fillRounded(hdc, rect{12, y, 204, y + 36}, rgb(10, 10, 10), 6)
			drawText(hdc, rect{28, y, 190, y + 36}, label, rgb(255, 255, 255), a.fontMedium, dtLeft|dtVCenter|dtSingleLine)
		} else {
			drawText(hdc, rect{28, y, 190, y + 36}, label, rgb(70, 70, 70), a.fontBody, dtLeft|dtVCenter|dtSingleLine)
		}
	}
	drawText(hdc, rect{24, a.height - 54, 190, a.height - 30}, "DH BOT 2.7.0", rgb(145, 145, 145), a.fontBody, dtLeft|dtVCenter|dtSingleLine)
}

func (a *uiApp) paintHeader(hdc uintptr) {
	x := int32(248)
	header := a.headerLayout()
	drawText(hdc, header.Title, pageTitles[a.page], rgb(10, 10, 10), a.fontHeading, dtLeft|dtVCenter|dtSingleLine|dtEndEllipsis)
	if a.page != pageSettings && a.page != pageDebug {
		button(hdc, header.Previous, "上一页", false)
		button(hdc, header.Next, "下一页", false)
		drawText(hdc, header.Page, a.pageLabel(), rgb(100, 100, 100), a.fontBody, dtCenter|dtVCenter|dtSingleLine|dtEndEllipsis)
	}
	status, detail, online, busy := a.readStatus()
	label := status
	if busy {
		label = "处理中"
	} else if online {
		label = "已连接"
	}
	statusPill(hdc, header.Status, label, status, online, busy)
	fill(hdc, rect{x, 76, a.width - 24, 77}, rgb(232, 232, 232))
	if detail != "" {
		drawText(hdc, rect{x, 78, a.width - 24, 100}, detail, rgb(90, 90, 90), a.fontBody, dtRight|dtVCenter|dtSingleLine|dtEndEllipsis)
	}
}

func (a *uiApp) paintOverview(hdc uintptr) {
	x := int32(248)
	status := appcore.Status{}
	if manager := a.currentManager(); manager != nil {
		status = manager.Snapshot()
	}
	metrics := []struct{ label, value string }{
		{"旺商聊", onOff(status.Online, "在线", "等待")},
		{"AI", onOff(status.AIReady, "就绪", "未配置")},
		{"启用群", strconv.Itoa(status.EnabledGroups)},
		{"已处理", strconv.FormatInt(status.ProcessedCount, 10)},
		{"自动动作", strconv.FormatInt(status.ActionCount, 10)},
	}
	content := a.width - x - 24
	width := (content - 4*12) / 5
	for index, metric := range metrics {
		left := x + int32(index)*(width+12)
		stroke(hdc, rect{left, 112, left + width, 204}, rgb(232, 232, 232))
		drawText(hdc, rect{left + 16, 126, left + width - 12, 150}, metric.label, rgb(100, 100, 100), a.fontBody, dtLeft|dtVCenter|dtSingleLine)
		drawText(hdc, rect{left + 16, 154, left + width - 12, 194}, metric.value, rgb(10, 10, 10), a.fontHeading, dtLeft|dtVCenter|dtSingleLine|dtEndEllipsis)
	}
	pauseLabel := "暂停全部自动化"
	if status.Paused {
		pauseLabel = "恢复全部自动化"
	}
	button(hdc, rect{a.width - 204, 224, a.width - 24, 262}, pauseLabel, status.Paused)
	sectionTitle(hdc, rect{x, 274, a.width - 24, 306}, "每日摘要（仅本机可见）", a.fontMedium)
	button(hdc, rect{a.width - 174, 274, a.width - 24, 306}, "查看所选摘要", false)
	sectionTitle(hdc, rect{x, 494, a.width - 24, 526}, "最近自动动作", a.fontMedium)
}

func (a *uiApp) paintGroups(hdc uintptr) {
	x := int32(248)
	ui := a.groupLayout()
	selected := groupmgr.Group{}
	if group, err := a.database.GetGroup(context.Background(), a.accountID, a.selectedGroup); err == nil {
		selected = group
	}
	fieldLabel(hdc, x+24, 100, "欢迎语（[成员] 会替换为群名片）")
	fieldLabel(hdc, a.width-258, 100, "每日摘要（本机时区 "+time.Now().Format("MST")+"）")
	button(hdc, ui.Sync, "同步群", false)
	button(hdc, ui.Enabled, "启用 / 停用", selected.Enabled)
	button(hdc, ui.AI, "AI 开关", selected.AIEnabled)
	button(hdc, ui.Moderation, "规则开关", selected.ModerationEnabled)
	button(hdc, ui.Manual, "人工接管", selected.ManualTakeover)
	button(hdc, ui.Tasks, "任务提醒", selected.TaskReminders)
	button(hdc, ui.Save, "保存群设置", false)
	button(hdc, ui.GroupMute, "全员禁言", false)
	button(hdc, ui.GroupUnmute, "解除全禁", false)
	button(hdc, ui.GroupTab, "群组", a.groupTab == 0)
	button(hdc, ui.MemberTab, "成员", a.groupTab == 1)
	selectedName := "请先选择群"
	if selected.GroupID != 0 {
		selectedName = "当前群：" + firstNonEmpty(selected.Name, "未命名群")
	}
	drawText(hdc, rect{ui.MemberTab.Right + 20, ui.GroupTab.Top, a.width - 24, ui.GroupTab.Bottom}, selectedName, rgb(90, 90, 90), a.fontBody, dtRight|dtVCenter|dtSingleLine|dtEndEllipsis)
	if a.groupTab == 0 {
		return
	}
	stats := appcore.CardStats{}
	if manager := a.currentManager(); manager != nil && a.selectedGroup != 0 {
		stats, _ = manager.CardStats(context.Background(), a.selectedGroup)
	}
	reported := stats.Settings.ReportedCount
	if reported == 0 {
		reported = stats.Present
	}
	button(hdc, ui.Auto, onOff(stats.Settings.AutoRename, "自动：开", "自动：关"), stats.Settings.AutoRename)
	button(hdc, ui.Preview, "生成建议", false)
	button(hdc, ui.Execute, "一键执行", true)
	button(hdc, ui.Pause, onOff(stats.Settings.Paused, "恢复", "暂停"), stats.Settings.Paused)
	button(hdc, ui.Retry, "重试失败", false)
	button(hdc, ui.Notices, fmt.Sprintf("新增%d", stats.UnreadNew), false)
	coverage := "完整"
	if stats.CoverageGap > 0 {
		coverage = fmt.Sprintf("缺口 %d", stats.CoverageGap)
	}
	sectionTitle(hdc, rect{x, ui.MemberListTop - 32, a.width - 24, ui.MemberListTop - 4}, fmt.Sprintf("成员（群 %d / 已识别 %d / %s / 待备注 %d / 失败 %d）", reported, stats.Settings.ResolvedCount, coverage, stats.Pending+stats.Running, stats.Failed), a.fontMedium)
	fieldLabel(hdc, x+24, ui.MemberPrimaryTop-28, "禁言分钟")
	fieldLabel(hdc, x+318, ui.MemberPrimaryTop-28, "选中成员两字简称")
	button(hdc, ui.MemberMute, "禁言", false)
	button(hdc, ui.MemberUnmute, "解禁", false)
	button(hdc, ui.MemberApply, "修改建议", false)
	button(hdc, ui.Cleanup, "清理封禁成员", false)
	restoreLabel := "恢复所有群员名称"
	if count := len(a.checkedMemberIDs()); count > 0 {
		restoreLabel = fmt.Sprintf("恢复该群员名称（%d）", count)
	}
	button(hdc, ui.Restore, restoreLabel, false)
	button(hdc, ui.Remove, "移出", true)
}

func (a *uiApp) paintMessages(hdc uintptr) {
	x := int32(248)
	fieldLabel(hdc, x+24, 100, "发送到群（可多选）")
	button(hdc, rect{x + 24, 128, x + 284, 164}, a.groupPickerLabel(pageMessages), false)
	fieldLabel(hdc, x+24, 176, "人工消息")
	button(hdc, rect{a.width - 154, 326, a.width - 24, 364}, "发送消息", true)
	sectionTitle(hdc, rect{x, 394, a.width - 24, 430}, "最近消息", a.fontMedium)
}

func (a *uiApp) paintRules(hdc uintptr) {
	x := int32(248)
	addLeft := a.width - 154
	recallX := addLeft - 200
	fieldLabel(hdc, x+24, 100, "权限校验群（规则全局生效）")
	button(hdc, rect{x + 24, 128, x + 284, 164}, a.groupPickerLabel(pageRules), false)
	fieldLabel(hdc, x+304, 100, "关键词 / 正则")
	fieldLabel(hdc, recallX, 100, "命中动作")
	button(hdc, rect{a.width - 154, 128, a.width - 24, 166}, "添加规则", true)
	fieldLabel(hdc, x+24, 176, "导入 / 导出 JSON 路径")
	button(hdc, rect{a.width - 254, 204, a.width - 154, 242}, "导入", false)
	button(hdc, rect{a.width - 142, 204, a.width - 24, 242}, "导出", false)
	button(hdc, rect{a.width - 374, 248, a.width - 274, 276}, "启用 / 停用", false)
	button(hdc, rect{a.width - 262, 248, a.width - 154, 276}, "切换模式", false)
	button(hdc, rect{a.width - 142, 248, a.width - 24, 276}, "删除所选", false)
}

func (a *uiApp) paintKnowledge(hdc uintptr) {
	x := int32(248)
	fieldLabel(hdc, x+24, 100, "知识库")
	button(hdc, rect{x + 24, 128, x + 284, 164}, a.knowledgeBaseLabel(), false)
	fieldLabel(hdc, x+304, 100, "标题")
	fieldLabel(hdc, x+584, 100, "TXT / Markdown 路径")
	fieldLabel(hdc, x+24, 176, "绑定群（可多选）")
	button(hdc, rect{x + 150, 174, x + 410, 208}, a.groupPickerLabel(pageKnowledge), false)
	button(hdc, rect{x + 430, 174, x + 550, 208}, "绑定群", false)
	button(hdc, rect{x + 560, 174, x + 680, 208}, "解除绑定", false)
	fieldLabel(hdc, x+24, 220, "知识库名称")
	fieldLabel(hdc, x+304, 220, "知识库说明")
	fieldLabel(hdc, x+24, 292, "群规 / FAQ 内容")
	button(hdc, rect{a.width - 550, 430, a.width - 430, 468}, "保存内容", true)
	button(hdc, rect{a.width - 418, 430, a.width - 298, 468}, "导入文档", false)
	button(hdc, rect{a.width - 286, 430, a.width - 166, 468}, "新建知识库", false)
	button(hdc, rect{a.width - 154, 430, a.width - 24, 468}, "复制为可编辑", false)
}

func (a *uiApp) paintTasks(hdc uintptr) {
	x := int32(248)
	right := a.width - 24
	dueX := right - 180
	assigneeX := dueX - 150
	fieldLabel(hdc, x+24, 100, "应用到群（可多选）")
	button(hdc, rect{x + 24, 128, x + 284, 164}, a.groupPickerLabel(pageTasks), false)
	fieldLabel(hdc, x+304, 100, "任务")
	fieldLabel(hdc, assigneeX, 100, "负责人内部 ID")
	fieldLabel(hdc, dueX, 100, "到期时间")
	drawText(hdc, rect{x + 24, 168, a.width - 24, 190}, "任务用来记录待办；到期后按最新群名片在群里提醒一次", rgb(95, 95, 95), a.fontBody, dtLeft|dtVCenter|dtSingleLine|dtEndEllipsis)
	button(hdc, rect{a.width - 286, 190, a.width - 166, 228}, "创建任务", true)
	button(hdc, rect{a.width - 154, 190, a.width - 24, 228}, "生成群摘要", false)
	sectionTitle(hdc, rect{x, 216, a.width - 24, 236}, "每日群发言计划", a.fontMedium)
	fieldLabel(hdc, x+24, 230, "计划名称")
	fieldLabel(hdc, x+304, 230, "开群时间")
	fieldLabel(hdc, x+424, 230, "关群时间")
	button(hdc, rect{x + 680, 254, x + 820, 290}, "保存计划", true)
	actionTop := a.bottomActionTop()
	button(hdc, rect{a.width - 154, actionTop, a.width - 24, actionTop + 38}, "完成所选", false)
}

func (a *uiApp) paintAudit(hdc uintptr) {
	x := int32(248)
	fieldLabel(hdc, x+24, 100, "查看群（可多选）")
	button(hdc, rect{x + 24, 128, x + 284, 164}, a.groupPickerLabel(pageAudit), false)
}

func (a *uiApp) paintSettings(hdc uintptr) {
	x := int32(248)
	labels := []struct {
		x, y int32
		text string
	}{
		{x + 24, 100, "旺商聊程序路径"},
		{x + 24, 200, "旺商聊 DevTools"}, {x + 454, 200, "内嵌桥地址"},
		{x + 24, 280, "AI 触发方式（固定）"}, {x + 294, 280, "AI 类型 openai/webhook"}, {x + 474, 280, "模型"},
		{x + 24, 360, "OpenAI-compatible Base URL"}, {x + 454, 360, "Webhook URL"},
		{x + 24, 440, "AI API Key（DPAPI 加密保存）"},
	}
	for _, label := range labels {
		fieldLabel(hdc, label.x, label.y, label.text)
	}
	button(hdc, rect{x + 24, 520, x + 154, 558}, "保存设置", true)
	button(hdc, rect{x + 166, 520, x + 296, 558}, "测试连接", false)
	button(hdc, rect{x + 308, 520, x + 438, 558}, "测试 AI", false)
	button(hdc, rect{x + 450, 520, x + 610, 558}, "启动 / 重启旺商聊", false)
	button(hdc, rect{x + 622, 520, x + 752, 558}, "打开调试", false)
	button(hdc, rect{x + 770, 520, x + 900, 558}, "恢复原文件", false)
	profileState := firstNonEmpty(a.wslProfileStatus.Detail, "首次启动旺商聊后自动建立固定登录分区")
	drawText(hdc, rect{x + 24, 168, x + 674, 198}, "登录状态："+profileState, rgb(100, 100, 100), a.fontBody, dtLeft|dtVCenter|dtSingleLine|dtEndEllipsis)
	prefixGroup := "未选择群"
	if group, err := a.database.GetGroup(context.Background(), a.accountID, a.selectedGroup); err == nil {
		prefixGroup = firstNonEmpty(group.Name, "未命名群")
	}
	fieldLabel(hdc, x+24, 576, "群名片前缀（"+prefixGroup+"）")
	drawText(hdc, rect{x + 216, 608, x + 370, 642}, "+ 群员0001…zzz9", rgb(105, 105, 105), a.fontBody, dtLeft|dtVCenter|dtSingleLine|dtEndEllipsis)
	button(hdc, rect{x + 382, 606, x + 496, 644}, "应用前缀", false)
	sectionTitle(hdc, rect{x, 650, a.width - 24, 674}, "运行信息", a.fontMedium)
	info := []string{
		"当前账号：" + firstNonEmpty(a.accountID, "等待识别"),
		"发送者 ID：" + idText(a.senderID),
		"数据库：" + store.DefaultPath(),
		"桥接：DH-BOT.exe 内嵌，DHBridge.exe 仅用于诊断",
	}
	for index, line := range info {
		top := int32(674 + index*20)
		drawText(hdc, rect{x, top, a.width - 24, top + 24}, line, rgb(70, 70, 70), a.fontBody, dtLeft|dtVCenter|dtSingleLine|dtEndEllipsis)
	}
}

func (a *uiApp) paintDebug(hdc uintptr) {
	x := int32(248)
	button(hdc, rect{x + 24, 112, x + 150, 150}, "立即检查", true)
	button(hdc, rect{x + 162, 112, x + 288, 150}, "返回设置", false)
	sectionTitle(hdc, rect{x, 178, a.width - 24, 208}, "连接诊断", a.fontMedium)
	a.mu.RLock()
	snapshot := a.debug
	a.mu.RUnlock()
	rows := []struct{ label, value, detail string }{
		{"旺商聊 DevTools", snapshot.DevTools, snapshot.DevToolsInfo},
		{"DH 内嵌桥", snapshot.Bridge, snapshot.BridgeInfo},
		{"协议会话 / NIM", snapshot.Session, snapshot.SessionInfo},
		{"旺商聊进程", snapshot.Process, snapshot.ProcessInfo},
		{"固定登录分区", snapshot.Profile, snapshot.ProfileInfo},
	}
	for index, row := range rows {
		top := int32(222 + index*54)
		stroke(hdc, rect{x, top, a.width - 24, top + 44}, rgb(232, 232, 232))
		drawText(hdc, rect{x + 14, top + 4, x + 210, top + 24}, row.label, rgb(75, 75, 75), a.fontMedium, dtLeft|dtVCenter|dtSingleLine)
		drawText(hdc, rect{x + 220, top + 4, x + 430, top + 24}, firstNonEmpty(row.value, "未检查"), rgb(10, 10, 10), a.fontMedium, dtLeft|dtVCenter|dtSingleLine|dtEndEllipsis)
		drawText(hdc, rect{x + 440, top + 4, a.width - 38, top + 38}, row.detail, rgb(100, 100, 100), a.fontBody, dtLeft|dtVCenter|dtSingleLine|dtEndEllipsis)
	}
	sectionTitle(hdc, rect{x, 512, a.width - 24, 542}, "诊断结论", a.fontMedium)
	advice := snapshot.Advice
	if advice == "" {
		advice = "点击“立即检查”读取 DevTools、内嵌桥、协议会话和旺商聊进程状态。"
	}
	fillRounded(hdc, rect{x, 554, a.width - 24, 648}, rgb(248, 248, 248), 6)
	drawText(hdc, rect{x + 16, 566, a.width - 40, 638}, advice, rgb(55, 55, 55), a.fontBody, dtLeft|dtVCenter|dtWordBreak)
	checked := "尚未检查"
	if !snapshot.CheckedAt.IsZero() {
		checked = "最近检查：" + snapshot.CheckedAt.Local().Format("2006-01-02 15:04:05")
	}
	drawText(hdc, rect{x, 666, a.width - 24, 692}, checked, rgb(125, 125, 125), a.fontBody, dtLeft|dtVCenter|dtSingleLine)
}

func (a *uiApp) click(x, y int32) {
	if x >= 12 && x <= 204 {
		for index := range navLabels {
			top := sidebarNavTop + int32(index*44)
			if y >= top && y <= top+36 {
				if oldList := a.currentListID(); oldList != 0 {
					a.searches[oldList] = strings.TrimSpace(a.getEdit(editSearch))
				}
				a.page = index
				if a.page == pageRules && a.selectedGroup != 0 {
					if manager := a.currentManager(); manager != nil {
						_ = manager.EnsureDefaultRules(context.Background(), a.selectedGroup)
					}
				}
				if newList := a.currentListID(); newList != 0 {
					a.setEdit(editSearch, a.searches[newList])
				}
				a.layoutControls()
				a.populateGroupEdits()
				a.refreshLists()
				if a.page == pageDebug {
					a.refreshDebug()
				}
				a.invalidate()
				return
			}
		}
	}
	header := a.headerLayout()
	if hit(x, y, header.Status) {
		a.connectNow()
		return
	}
	contentX := int32(248)
	if a.page != pageSettings && a.page != pageDebug {
		id := a.currentListID()
		if hit(x, y, header.Previous) {
			if a.listPages[id] > 0 {
				a.listPages[id]--
			}
			a.refreshLists()
			a.invalidate()
			return
		}
		if hit(x, y, header.Next) {
			pageSize := listPageSize(id)
			pages := (a.listTotals[id] + pageSize - 1) / pageSize
			if pages < 1 {
				pages = 1
			}
			if a.listPages[id]+1 < pages {
				a.listPages[id]++
			}
			a.refreshLists()
			a.invalidate()
			return
		}
	}
	a.syncSelectionsFromLists()
	switch a.page {
	case pageOverview:
		if hit(x, y, rect{a.width - 204, 224, a.width - 24, 262}) {
			a.toggleGlobalPause()
		}
		if hit(x, y, rect{a.width - 174, 274, a.width - 24, 306}) {
			a.showSelectedSummary()
		}
	case pageGroups:
		ui := a.groupLayout()
		switch {
		case hit(x, y, ui.Sync):
			a.syncNow()
		case hit(x, y, ui.Enabled):
			a.toggleGroup("enabled")
		case hit(x, y, ui.AI):
			a.toggleGroup("ai")
		case hit(x, y, ui.Moderation):
			a.toggleGroup("moderation")
		case hit(x, y, ui.Manual):
			a.toggleGroup("manual")
		case hit(x, y, ui.Tasks):
			a.toggleGroup("tasks")
		case hit(x, y, ui.Save):
			a.saveWelcome()
		case hit(x, y, ui.GroupMute):
			a.groupMute(true)
		case hit(x, y, ui.GroupUnmute):
			a.groupMute(false)
		case hit(x, y, ui.GroupTab):
			a.switchGroupTab(0)
		case hit(x, y, ui.MemberTab):
			a.switchGroupTab(1)
		case a.groupTab == 1 && hit(x, y, ui.Auto):
			a.saveCardConfiguration("auto")
		case a.groupTab == 1 && hit(x, y, ui.Preview):
			a.previewCards()
		case a.groupTab == 1 && hit(x, y, ui.Execute):
			a.queueCards()
		case a.groupTab == 1 && hit(x, y, ui.Pause):
			a.saveCardConfiguration("pause")
		case a.groupTab == 1 && hit(x, y, ui.Retry):
			a.retryCards()
		case a.groupTab == 1 && hit(x, y, ui.Notices):
			a.readCardNotices()
		}
		switch {
		case a.groupTab == 1 && hit(x, y, ui.MemberMute):
			a.memberAction("mute")
		case a.groupTab == 1 && hit(x, y, ui.MemberUnmute):
			a.memberAction("unmute")
		case a.groupTab == 1 && hit(x, y, ui.MemberApply):
			a.applyCardSuggestion()
		case a.groupTab == 1 && hit(x, y, ui.Cleanup):
			a.cleanupInactiveMembers()
		case a.groupTab == 1 && hit(x, y, ui.Restore):
			a.restoreMemberNames()
		case a.groupTab == 1 && hit(x, y, ui.Remove):
			a.memberAction("remove")
		}
	case pageMessages:
		if hit(x, y, rect{contentX + 24, 128, contentX + 284, 164}) {
			a.showGroupMenu(pageMessages)
		} else if hit(x, y, rect{a.width - 154, 326, a.width - 24, 364}) {
			a.sendManual()
		}
	case pageRules:
		switch {
		case hit(x, y, rect{contentX + 24, 128, contentX + 284, 164}):
			a.showGroupMenu(pageRules)
		case hit(x, y, rect{a.width - 154, 128, a.width - 24, 166}):
			a.addRule()
		case hit(x, y, rect{a.width - 254, 204, a.width - 154, 242}):
			a.importRules()
		case hit(x, y, rect{a.width - 142, 204, a.width - 24, 242}):
			a.exportRules()
		case hit(x, y, rect{a.width - 374, 248, a.width - 274, 276}):
			a.manageRule("enabled")
		case hit(x, y, rect{a.width - 262, 248, a.width - 154, 276}):
			a.manageRule("mode")
		case hit(x, y, rect{a.width - 142, 248, a.width - 24, 276}):
			a.manageRule("delete")
		}
	case pageKnowledge:
		if hit(x, y, rect{contentX + 24, 128, contentX + 284, 164}) {
			a.showKnowledgeBaseMenu()
		} else if hit(x, y, rect{contentX + 150, 174, contentX + 410, 208}) {
			a.showGroupMenu(pageKnowledge)
		} else if hit(x, y, rect{contentX + 430, 174, contentX + 550, 208}) {
			a.bindKnowledgeGroups(true)
		} else if hit(x, y, rect{contentX + 560, 174, contentX + 680, 208}) {
			a.bindKnowledgeGroups(false)
		} else if hit(x, y, rect{a.width - 550, 430, a.width - 430, 468}) {
			a.saveKnowledge()
		}
		if hit(x, y, rect{a.width - 418, 430, a.width - 298, 468}) {
			a.importKnowledge()
		}
		if hit(x, y, rect{a.width - 286, 430, a.width - 166, 468}) {
			a.createKnowledgeBase()
		}
		if hit(x, y, rect{a.width - 154, 430, a.width - 24, 468}) {
			a.copyKnowledgeBase()
		}
	case pageTasks:
		if hit(x, y, rect{contentX + 24, 128, contentX + 284, 164}) {
			a.showGroupMenu(pageTasks)
		} else if hit(x, y, rect{a.width - 286, 190, a.width - 166, 228}) {
			a.createTask()
		}
		if hit(x, y, rect{a.width - 154, 190, a.width - 24, 228}) {
			a.summarize()
		}
		if hit(x, y, rect{contentX + 680, 254, contentX + 820, 290}) {
			a.saveGroupSchedule()
		}
		actionTop := a.bottomActionTop()
		if hit(x, y, rect{a.width - 154, actionTop, a.width - 24, actionTop + 38}) {
			a.completeTask()
		}
	case pageAudit:
		if hit(x, y, rect{contentX + 24, 128, contentX + 284, 164}) {
			a.showGroupMenu(pageAudit)
		}
	case pageSettings:
		if hit(x, y, rect{contentX + 24, 520, contentX + 154, 558}) {
			a.saveSettings()
		}
		if hit(x, y, rect{contentX + 166, 520, contentX + 296, 558}) {
			a.connectNow()
		}
		if hit(x, y, rect{contentX + 308, 520, contentX + 438, 558}) {
			a.testAI()
		}
		if hit(x, y, rect{contentX + 450, 520, contentX + 610, 558}) {
			a.startOrRestartWangShangLiao()
		}
		if hit(x, y, rect{contentX + 622, 520, contentX + 752, 558}) {
			a.page = pageDebug
			a.layoutControls()
			a.refreshDebug()
		}
		if hit(x, y, rect{contentX + 770, 520, contentX + 900, 558}) {
			a.restoreWangShangLiaoProfile()
		}
		if hit(x, y, rect{contentX + 382, 606, contentX + 496, 644}) {
			a.saveCardConfiguration("prefix")
		}
	case pageDebug:
		if hit(x, y, rect{contentX + 24, 112, contentX + 150, 150}) {
			a.refreshDebug()
		} else if hit(x, y, rect{contentX + 162, 112, contentX + 288, 150}) {
			a.page = pageSettings
			a.layoutControls()
			a.invalidate()
		}
	}
	a.invalidate()
}

func (a *uiApp) hover(x, y int32) {
	a.mouseInside = true
	tracking := trackMouseEvent{Size: uint32(unsafe.Sizeof(trackMouseEvent{})), Flags: tmeLeave, HWndTrack: a.hwnd}
	procTrackMouseEvent.Call(uintptr(unsafe.Pointer(&tracking)))
	text := ""
	contentX := int32(248)
	if a.page == pageGroups {
		ui := a.groupLayout()
		group, _ := a.database.GetGroup(context.Background(), a.accountID, a.selectedGroup)
		switch {
		case hit(x, y, ui.Sync):
			text = "刷新群列表和当前选中群成员，不发送欢迎消息"
		case hit(x, y, ui.Enabled):
			text = featureDetail(group.Enabled, "允许 DH 处理当前群的消息、任务和自动化")
		case hit(x, y, ui.AI):
			text = featureDetail(group.AIEnabled, "允许 AI 问答和本地每日摘要")
		case hit(x, y, ui.Moderation):
			text = featureDetail(group.ModerationEnabled, "按规则执行撤回、禁言和其他群管动作")
		case hit(x, y, ui.Manual):
			text = featureDetail(group.ManualTakeover, "暂停当前群的 AI 自动回复，确定性规则继续运行")
		case hit(x, y, ui.Tasks):
			text = featureDetail(group.TaskReminders, "在任务到期时执行群内提醒")
		case a.groupTab == 1 && hit(x, y, ui.Auto):
			settings, _ := a.database.GetCardSettings(context.Background(), a.accountID, a.selectedGroup)
			text = featureDetail(settings.AutoRename, "新成员入群后自动生成并更新群名片")
		case a.groupTab == 1 && hit(x, y, ui.Preview):
			text = "同步当前群并重新生成建议名称"
		case a.groupTab == 1 && hit(x, y, ui.Execute):
			text = "将当前预览中的建议名称加入后台执行队列"
		case a.groupTab == 1 && hit(x, y, ui.Cleanup):
			text = "识别注销和封禁状态，确认后移出普通成员"
		case a.groupTab == 1 && hit(x, y, ui.Restore):
			text = "把全部或已勾选成员恢复为 DH 首次保存的原名称"
		}
	} else if a.page == pageSettings && hit(x, y, rect{contentX + 382, 586, contentX + 496, 624}) {
		text = "把前缀应用到当前选中群，四位固定后缀保持不变"
	} else if a.page == pageSettings && hit(x, y, rect{contentX + 622, 500, contentX + 752, 538}) {
		text = "打开连接诊断，检查 DevTools、内嵌桥、协议会话和 NIM 初始化状态"
	} else if a.page == pageDebug {
		if hit(x, y, rect{contentX + 24, 112, contentX + 150, 150}) {
			text = "重新读取本机连接链路，不执行群管理动作"
		} else if hit(x, y, rect{contentX + 162, 112, contentX + 288, 150}) {
			text = "返回设置页"
		}
	} else if a.page == pageTasks {
		if hit(x, y, rect{a.width - 286, 190, a.width - 166, 228}) {
			text = "保存待办任务，到期后由任务提醒开关控制是否发送群提醒"
		} else if hit(x, y, rect{a.width - 154, 190, a.width - 24, 228}) {
			text = "生成只保存在总览的私密每日摘要"
		} else if hit(x, y, rect{contentX + 680, 254, contentX + 820, 290}) {
			text = "保存每日开群和关群时间；开群允许成员发言，关群执行全员禁言"
		}
	}
	if text == a.hoverText {
		return
	}
	a.hoverText, a.hoverX, a.hoverY = text, x, y
	a.invalidate()
}

func featureDetail(enabled bool, description string) string {
	state := "「已关闭」"
	if enabled {
		state = "「已开启」"
	}
	return state + description
}

func (a *uiApp) paintTooltip(hdc uintptr) {
	if a.hoverText == "" {
		return
	}
	width, height := int32(380), int32(54)
	left, top := a.hoverX+16, a.hoverY+20
	if left+width > a.width-12 {
		left = a.width - width - 12
	}
	if top+height > a.height-12 {
		top = a.hoverY - height - 12
	}
	area := rect{left, top, left + width, top + height}
	fillRounded(hdc, area, rgb(20, 20, 20), 6)
	drawText(hdc, rect{left + 12, top + 7, left + width - 12, top + height - 7}, a.hoverText, rgb(255, 255, 255), a.fontBody, dtLeft|dtVCenter|dtWordBreak)
}

func (a *uiApp) backgroundLoop() {
	pollTicker := time.NewTicker(500 * time.Millisecond)
	cleanupTicker := time.NewTicker(24 * time.Hour)
	defer pollTicker.Stop()
	defer cleanupTicker.Stop()
	for {
		select {
		case <-a.stop:
			return
		case <-cleanupTicker.C:
			_ = a.database.Cleanup(context.Background(), time.Now())
		case <-pollTicker.C:
			if time.Since(a.lastConnect) >= 5*time.Second {
				a.tryConnect()
			}
			if manager := a.currentManager(); manager != nil && manager.Snapshot().Online {
				a.pollMessages(manager)
				a.runCardWorker(manager)
				if time.Since(a.lastMemberSync) >= time.Minute {
					a.syncEnabledMembers(manager)
				}
				if time.Since(a.lastScheduled) >= 30*time.Second {
					a.lastScheduled = time.Now()
					a.launch("定时任务", func() {
						ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
						defer cancel()
						if err := manager.RunScheduled(ctx, time.Now()); err != nil {
							a.setResult("定时任务：" + userFacingError(err))
						}
					})
				}
			}
		}
	}
}

func (a *uiApp) runCardWorker(manager *appcore.Manager) {
	a.mu.Lock()
	if a.cardWorkerRunning {
		a.mu.Unlock()
		return
	}
	a.cardWorkerRunning = true
	a.mu.Unlock()
	a.launch("群名片任务", func() {
		ctx, cancel := context.WithTimeout(context.Background(), 25*time.Second)
		_, err := manager.RunCardJobs(ctx, 1)
		cancel()
		a.mu.Lock()
		a.cardWorkerRunning = false
		if err != nil {
			a.detail = "群名片任务: " + userFacingError(err)
		}
		a.mu.Unlock()
		procPostMessageW.Call(a.hwnd, wmAppRefresh, 0, 0)
	})
}

func (a *uiApp) tryConnect() {
	a.mu.Lock()
	a.lastConnect = time.Now()
	a.mu.Unlock()
	ctx, cancel := context.WithTimeout(context.Background(), 4*time.Second)
	defer cancel()
	if a.wslController != nil && a.setting("wsl.auto_start", "true") != "false" {
		if err := a.ensureWangShangLiao(ctx, false); err != nil {
			a.setConnection("等待旺商聊", err.Error(), false)
			return
		}
	}
	endpoint := a.setting("bridge.endpoint", "http://127.0.0.1:51235")
	if err := urlguard.ValidateLoopback(endpoint); err != nil {
		a.setConnection("连接设置异常", err.Error(), false)
		return
	}
	client := protocol.NewClient(endpoint, protocol.Credentials{})
	if _, err := client.Ping(ctx); err != nil {
		a.setConnection("等待旺商聊", userFacingError(err), false)
		return
	}
	info, _, err := client.SessionInfo(ctx)
	if err != nil || info.SenderID == 0 {
		a.setConnection("等待登录", errorText(err), false)
		return
	}
	accountID := strconv.FormatInt(info.SenderID, 10)
	a.mu.Lock()
	if a.manager == nil || a.accountID != accountID {
		a.client = client
		a.gateway = protocol.NewHTTPGateway(client)
		a.gateway.SetSessionIdentity(info.SenderID, info.NIMAccount)
		a.manager = appcore.New(a.database, a.gateway, accountID)
		a.manager.SetPaused(a.setting("automation.paused", "false") == "true")
		a.accountID, a.senderID = accountID, info.SenderID
		a.selectedKnowledgeBase = 0
		a.groupSelectionReady = false
		a.configureProviderLocked()
	}
	manager := a.manager
	a.mu.Unlock()
	_ = a.database.MigrateLegacyKnowledge(ctx, accountID)
	a.refreshKnowledgeBases()
	if err := manager.SyncGroups(ctx); err != nil {
		a.setConnection("同步失败", userFacingError(err), false)
		return
	}
	groups, _ := a.database.ListGroups(ctx, accountID, false)
	if !a.groupSelectionReady && len(groups) > 0 {
		a.selectedGroup = groups[0].GroupID
		preferredGroup := parseID(a.setting("testing.group_id", ""), 0)
		for _, group := range groups {
			if preferredGroup != 0 && group.GroupID == preferredGroup {
				a.selectedGroup = group.GroupID
				break
			}
		}
		if preferredGroup == 0 {
			for _, group := range groups {
				if a.legacyGroup != 0 && group.GroupID == a.legacyGroup {
					a.selectedGroup = group.GroupID
					break
				}
			}
		}
		if preferredGroup == 0 && a.legacyGroup == 0 {
			for _, group := range groups {
				if group.Enabled {
					a.selectedGroup = group.GroupID
					break
				}
			}
		}
		a.groupSelectionReady = true
		a.populateGroupEdits()
	}
	manager.SetOnline(true, nil)
	a.setConnection("已连接", fmt.Sprintf("账号 %d · %d 个群", info.SenderID, info.GroupCount), true)
}

func (a *uiApp) pollMessages(manager *appcore.Manager) {
	a.mu.RLock()
	client, ownID := a.client, a.senderID
	a.mu.RUnlock()
	if client == nil {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
	defer cancel()
	batch, _, err := client.PeekMessages(ctx)
	if err != nil || !batch.OK {
		manager.SetOnline(false, err)
		a.setConnection("连接中断", errorText(err), false)
		return
	}
	lastAck := uint64(0)
	ackMessageIDs := make([]int64, 0)
	for _, incoming := range batch.Messages {
		if incoming.Seq == 0 {
			continue
		}
		if joined, ok := protocol.ParseTeamJoinNotification(incoming); ok {
			a.mu.RLock()
			gateway := a.gateway
			a.mu.RUnlock()
			if gateway == nil {
				break
			}
			groupID, resolveErr := gateway.ResolveGroupID(ctx, joined.GroupCloudID)
			if resolveErr != nil {
				a.setResult("入群通知解析失败: " + resolveErr.Error())
				break
			}
			if joinErr := manager.HandleMemberJoined(ctx, groupID); joinErr != nil {
				a.setResult("入群成员同步失败: " + joinErr.Error())
				break
			}
			lastAck = incoming.Seq
			continue
		}
		if incoming.Flow == "out" || incoming.Decoded == nil || incoming.DecodeError != "" || incoming.Decoded.MsgSession != 2 || incoming.Decoded.From.ID == ownID {
			lastAck = incoming.Seq
			continue
		}
		kind := groupmgr.MessageNotice
		switch incoming.Decoded.MsgFormat {
		case 0:
			kind = groupmgr.MessageText
		case 1:
			kind = groupmgr.MessageImage
		case 13:
			kind = groupmgr.MessageCard
		default:
			kind = groupmgr.MessageOther
		}
		messageID := firstNonEmpty(incoming.IDServer, incoming.IDClient)
		processed, processErr := manager.ProcessAsync(ctx, appcore.Incoming{
			Sequence: incoming.Seq, ServerMessageID: messageID, GroupID: incoming.Decoded.To.ID,
			UserID: incoming.Decoded.From.ID, SenderName: firstNonEmpty(incoming.Decoded.From.Name, incoming.FromNick),
			Kind: kind, Text: incoming.Decoded.Content.Data, SentAt: nimTime(incoming.Time),
		})
		if processErr != nil {
			a.setResult("消息处理失败: " + processErr.Error())
			break
		}
		lastAck = incoming.Seq
		if processed.MessageID != 0 {
			ackMessageIDs = append(ackMessageIDs, processed.MessageID)
		}
	}
	if lastAck != 0 {
		if _, _, err := client.AckMessages(ctx, lastAck); err == nil {
			for _, id := range ackMessageIDs {
				_ = a.database.AcknowledgeMessage(ctx, id)
			}
		}
	}
}

func (a *uiApp) syncEnabledMembers(manager *appcore.Manager) {
	a.lastMemberSync = time.Now()
	groups, _ := a.database.ListGroups(context.Background(), a.accountID, true)
	for _, group := range groups {
		ctx, cancel := context.WithTimeout(context.Background(), 12*time.Second)
		_ = manager.SyncMembers(ctx, group.GroupID)
		cancel()
	}
}

func (a *uiApp) startBridge() {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.bridgeService != nil {
		_ = a.bridgeService.Close()
	}
	devToolsURL := a.getEditOrSetting(editDevTools, "bridge.devtools", embeddedbridge.DefaultDevToolsURL)
	if err := urlguard.ValidateLoopback(devToolsURL); err != nil {
		a.detail = err.Error()
		return
	}
	service, err := embeddedbridge.Start(context.Background(), embeddedbridge.Options{
		Address:     embeddedbridge.DefaultAddress,
		DevToolsURL: devToolsURL,
	})
	if err != nil {
		a.detail = userFacingError(err)
		return
	}
	a.bridgeService = service
	if service.Reused {
		a.detail = "复用已有本地桥"
	}
}

func (a *uiApp) ensureWangShangLiao(ctx context.Context, manual bool) error {
	if a.wslController == nil {
		return nil
	}
	devtools := a.getEditOrSetting(editDevTools, "bridge.devtools", embeddedbridge.DefaultDevToolsURL)
	status, err := a.wslController.Inspect(ctx, devtools)
	if err == nil && status == wslcontrol.DevToolsReady {
		if manual || (a.manager == nil && !a.wslActivatedNIM) {
			path := a.wslExecutablePath()
			if path == "" {
				if candidates, locateErr := a.wslController.Locate(ctx); locateErr == nil && len(candidates) == 1 {
					path = candidates[0].Path
					a.wslPath = path
				}
			}
			if _, activateErr := a.wslController.Activate(ctx, path); activateErr == nil {
				a.wslActivatedNIM = true
			}
		}
		return nil
	}
	if status == wslcontrol.DevToolsOtherService {
		return errors.New("9222 端口被其他程序占用")
	}
	path := strings.TrimSpace(a.setting("wsl.path", ""))
	if path == "" {
		candidates, locateErr := a.wslController.Locate(ctx)
		if locateErr != nil || len(candidates) == 0 {
			return fmt.Errorf("未找到旺商聊，请在设置中填写程序路径")
		}
		if len(candidates) > 1 {
			return fmt.Errorf("发现多个旺商聊安装位置，请在设置中选择路径")
		}
		path = candidates[0].Path
	}
	a.wslPath = path
	processes, listErr := a.wslController.ListProcesses(ctx, path)
	if listErr != nil {
		return listErr
	}
	if len(processes) > 0 {
		if !manual && a.wslPrompted {
			return errors.New("旺商聊已运行但未开启 9222 DevTools")
		}
		a.wslPrompted = true
		message := "检测到旺商聊已运行，但未开启 9222 DevTools。\n\nDH 需要结束该旺商聊进程并重新启动，是否继续？"
		answer, _, _ := procMessageBoxW.Call(a.hwnd, uintptr(unsafe.Pointer(utf16(message))), uintptr(unsafe.Pointer(utf16("DH BOT"))), mbYesNo|mbIconWarning)
		if answer != idYes {
			return errors.New("已取消重启旺商聊")
		}
		for _, process := range processes {
			if err := a.wslController.Stop(ctx, process); err != nil {
				return err
			}
		}
		time.Sleep(3 * time.Second)
	}
	if a.setting("wsl.persist_login", "true") != "false" {
		profile, profileErr := a.wslController.PreparePersistentProfile(ctx, path, firstNonEmpty(a.setting("wsl.profile_id", ""), defaultWangProfile))
		a.wslProfileStatus = profile
		if profileErr != nil {
			return fmt.Errorf("旺商聊登录分区准备失败：%w", profileErr)
		}
		_ = a.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: "wsl.profile_status", Value: profile.Detail, UpdatedAt: time.Now().UTC()})
		_ = a.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: "wsl.profile_state", Value: profile.State, UpdatedAt: time.Now().UTC()})
		_ = a.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: "wsl.profile_script", Value: profile.ScriptPath, UpdatedAt: time.Now().UTC()})
		_ = a.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: "wsl.profile_backup", Value: profile.BackupPath, UpdatedAt: time.Now().UTC()})
	}
	if _, err := a.wslController.Start(ctx, wslcontrol.StartOptions{ImagePath: path, DevToolsURL: devtools, ProfileID: firstNonEmpty(a.setting("wsl.profile_id", ""), defaultWangProfile)}); err != nil {
		return err
	}
	_ = a.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: "wsl.path", Value: path, UpdatedAt: time.Now().UTC()})
	deadline := time.Now().Add(15 * time.Second)
	for time.Now().Before(deadline) {
		if state, _ := a.wslController.Inspect(ctx, devtools); state == wslcontrol.DevToolsReady {
			a.wslPrompted = false
			a.wslActivatedNIM = true
			_, _ = a.wslController.Activate(ctx, path)
			return nil
		}
		time.Sleep(500 * time.Millisecond)
	}
	return errors.New("旺商聊已启动，但 9222 DevTools 尚未就绪")
}

func (a *uiApp) wslExecutablePath() string {
	path := strings.TrimSpace(a.setting("wsl.path", ""))
	if path != "" {
		return path
	}
	return strings.TrimSpace(a.wslPath)
}

func (a *uiApp) restoreWangShangLiaoProfile() {
	if a.wslController == nil {
		a.setResult("当前系统不支持旺商聊登录分区管理")
		return
	}
	path := a.wslExecutablePath()
	backup := a.wslProfileStatus.BackupPath
	if path == "" || backup == "" {
		a.setResult("尚未找到旺商聊原文件备份")
		return
	}
	a.async("恢复旺商聊原文件", func() error {
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		processes, err := a.wslController.ListProcesses(ctx, path)
		if err != nil {
			return err
		}
		if len(processes) > 0 {
			return errors.New("请先退出旺商聊，再恢复原文件")
		}
		if err := a.wslController.RestorePersistentProfile(ctx, a.wslProfileStatus.ScriptPath, backup); err != nil {
			return err
		}
		a.wslProfileStatus.State = "restored"
		a.wslProfileStatus.Detail = "已恢复旺商聊原启动文件"
		_ = a.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: "wsl.profile_state", Value: "restored", UpdatedAt: time.Now().UTC()})
		_ = a.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: "wsl.profile_status", Value: a.wslProfileStatus.Detail, UpdatedAt: time.Now().UTC()})
		return nil
	})
}

func (a *uiApp) startOrRestartWangShangLiao() {
	a.async("启动 / 重启旺商聊", func() error {
		return a.ensureWangShangLiao(context.Background(), true)
	})
}

func (a *uiApp) refreshDebug() {
	a.async("立即检查", func() error {
		ctx, cancel := context.WithTimeout(context.Background(), 12*time.Second)
		defer cancel()
		snapshot := a.collectDebug(ctx)
		a.mu.Lock()
		a.debug = snapshot
		a.mu.Unlock()
		return nil
	})
}

func (a *uiApp) collectDebug(ctx context.Context) debugSnapshot {
	snapshot := debugSnapshot{CheckedAt: time.Now()}
	devtools := a.getEditOrSetting(editDevTools, "bridge.devtools", embeddedbridge.DefaultDevToolsURL)
	if body, status, err := debugGET(ctx, strings.TrimRight(devtools, "/")+"/json/version"); err != nil {
		snapshot.DevTools, snapshot.DevToolsInfo = "不可访问", userFacingError(err)
	} else {
		var version map[string]any
		_ = json.Unmarshal(body, &version)
		browser := firstNonEmpty(debugStringValue(version["Browser"]), debugStringValue(version["User-Agent"]))
		snapshot.DevTools = "可访问"
		snapshot.DevToolsInfo = fmt.Sprintf("HTTP %d · %s", status, firstNonEmpty(browser, "DevTools 已响应"))
		if pages, pageStatus, pageErr := debugGET(ctx, strings.TrimRight(devtools, "/")+"/json/list"); pageErr == nil {
			var list []struct {
				Type  string `json:"type"`
				Title string `json:"title"`
				URL   string `json:"url"`
			}
			if json.Unmarshal(pages, &list) == nil {
				titles := make([]string, 0, 2)
				for _, page := range list {
					if page.Type == "page" && len(titles) < 2 {
						titles = append(titles, firstNonEmpty(page.Title, page.URL, "未命名页面"))
					}
				}
				snapshot.DevToolsInfo = fmt.Sprintf("HTTP %d · 页面 %d · %s", pageStatus, len(titles), strings.Join(titles, " / "))
			}
		}
	}

	endpoint := a.getEditOrSetting(editEndpoint, "bridge.endpoint", "http://127.0.0.1:51235")
	client := protocol.NewClient(endpoint, protocol.Credentials{})
	if result, err := client.Ping(ctx); err != nil {
		snapshot.Bridge, snapshot.BridgeInfo = "不可用", userFacingError(err)
	} else {
		snapshot.Bridge = "正常"
		snapshot.BridgeInfo = fmt.Sprintf("HTTP %d · DH 内嵌桥响应", result.HTTPStatus)
	}
	if info, _, err := client.SessionInfo(ctx); err != nil {
		snapshot.Session, snapshot.SessionInfo = "未就绪", userFacingError(err)
	} else {
		snapshot.Session = "已就绪"
		snapshot.SessionInfo = fmt.Sprintf("账号 %d · NIM %s · %d 个群", info.SenderID, firstNonEmpty(info.NIMAccount, "已连接"), info.GroupCount)
	}

	path := strings.TrimSpace(a.setting("wsl.path", ""))
	a.mu.RLock()
	profileStatus := a.wslProfileStatus
	a.mu.RUnlock()
	snapshot.Profile = firstNonEmpty(profileStatus.State, onOff(a.setting("wsl.persist_login", "true") != "false", "开启", "关闭"))
	snapshot.ProfileInfo = firstNonEmpty(profileStatus.Detail, "固定分区："+firstNonEmpty(a.setting("wsl.profile_id", ""), defaultWangProfile))
	if a.wslController == nil {
		snapshot.Process, snapshot.ProcessInfo = "仅 Windows", "当前系统不启用 Windows 进程控制"
	} else {
		if path == "" {
			if candidates, err := a.wslController.Locate(ctx); err == nil && len(candidates) == 1 {
				path = candidates[0].Path
			} else if len(candidates) > 1 {
				snapshot.Process, snapshot.ProcessInfo = "需选择", fmt.Sprintf("发现 %d 个候选安装路径，请在设置页填写准确路径", len(candidates))
			}
		}
		if path != "" {
			processes, err := a.wslController.ListProcesses(ctx, path)
			if err != nil {
				snapshot.Process, snapshot.ProcessInfo = "查询失败", userFacingError(err)
			} else {
				snapshot.Process = onOff(len(processes) > 0, "运行中", "未运行")
				snapshot.ProcessInfo = fmt.Sprintf("%s · PID %s", path, processIDs(processes))
			}
		} else if snapshot.Process == "" {
			snapshot.Process, snapshot.ProcessInfo = "未定位", "设置页填写旺商聊主程序路径后可自动启动"
		}
	}

	switch {
	case snapshot.DevTools != "可访问":
		snapshot.Advice = "先确认旺商聊已使用 9222 DevTools 启动。设置页的 DevTools 地址必须与实际端口一致。"
	case snapshot.Session == "未就绪":
		snapshot.Advice = "DevTools 已打开但 NIM 尚未初始化。请先在旺商聊完成登录，进入主界面后等待几秒，再点击“立即检查”；如果仍未就绪，使用“启动 / 重启旺商聊”确认重启。"
	case snapshot.Bridge != "正常":
		snapshot.Advice = "DH 内嵌桥没有正常响应。退出重复运行的 DH 后重新打开，或检查 51235 是否被其他本地程序占用。"
	default:
		snapshot.Advice = "连接链路已通过基础检查，可以继续同步群列表。"
	}
	return snapshot
}

func debugGET(ctx context.Context, rawURL string) ([]byte, int, error) {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, rawURL, nil)
	if err != nil {
		return nil, 0, err
	}
	response, err := (&http.Client{Timeout: 3 * time.Second}).Do(request)
	if err != nil {
		return nil, 0, err
	}
	defer response.Body.Close()
	body, err := io.ReadAll(io.LimitReader(response.Body, 1<<20))
	if err != nil {
		return nil, response.StatusCode, err
	}
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		return body, response.StatusCode, fmt.Errorf("HTTP %d", response.StatusCode)
	}
	return body, response.StatusCode, nil
}

func processIDs(processes []wslcontrol.ProcessRef) string {
	if len(processes) == 0 {
		return "无"
	}
	ids := make([]string, 0, len(processes))
	for _, process := range processes {
		ids = append(ids, strconv.Itoa(process.PID))
	}
	return strings.Join(ids, ",")
}

func debugStringValue(value any) string {
	if value == nil {
		return ""
	}
	if text, ok := value.(string); ok {
		return strings.TrimSpace(text)
	}
	return strings.TrimSpace(fmt.Sprint(value))
}

func (a *uiApp) loadLegacy() {
	base, _ := os.UserConfigDir()
	path := filepath.Join(base, "DH", "state.json")
	if value := a.setting("migration.default_group", ""); value != "" {
		a.legacyGroup, _ = strconv.ParseInt(value, 10, 64)
	}
	connection, exists, err := migration.ReadLegacy(path)
	if err != nil {
		a.detail = userFacingError(err)
		return
	}
	if !exists {
		return
	}
	if connection.Endpoint != "" {
		_ = a.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: "bridge.endpoint", Value: connection.Endpoint, UpdatedAt: time.Now().UTC()})
	}
	a.legacyGroup = connection.DefaultGroupID
	if connection.DefaultGroupID != 0 {
		_ = a.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: "migration.default_group", Value: strconv.FormatInt(connection.DefaultGroupID, 10), UpdatedAt: time.Now().UTC()})
	}
	if err := migration.RemoveLegacy(path); err != nil {
		a.detail = userFacingError(err)
	}
}

func (a *uiApp) syncNow() {
	a.async("同步群", func() error {
		manager := a.currentManager()
		if manager == nil {
			return fmt.Errorf("旺商聊尚未连接")
		}
		if err := manager.SyncGroups(context.Background()); err != nil {
			return err
		}
		if a.selectedGroup == 0 {
			return fmt.Errorf("请先选择群")
		}
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		err := manager.SyncMembers(ctx, a.selectedGroup)
		cancel()
		if err != nil {
			return err
		}
		a.lastMemberSync = time.Now()
		return nil
	})
}

func (a *uiApp) saveCardConfiguration(action string) {
	manager := a.currentManager()
	if manager == nil || a.selectedGroup == 0 {
		a.setResult("请先选择群并连接旺商聊")
		return
	}
	settings, err := a.database.GetCardSettings(context.Background(), a.accountID, a.selectedGroup)
	if err != nil {
		a.report(err)
		return
	}
	prefix := strings.TrimSpace(a.getEdit(editCardPrefix))
	if prefix == "" {
		prefix = "DH"
		a.setEdit(editCardPrefix, prefix)
	}
	autoRename, paused := settings.AutoRename, settings.Paused
	switch action {
	case "auto":
		autoRename = !autoRename
	case "pause":
		paused = !paused
	}
	updateExisting := false
	if prefix != settings.Prefix && a.hasNumberedMembers() {
		var accepted bool
		updateExisting, accepted = a.prefixChangeChoice(settings.Prefix)
		if !accepted {
			return
		}
	}
	label := "应用前缀"
	if action == "auto" {
		label = onOff(settings.AutoRename, "自动：开", "自动：关")
	}
	if action == "pause" {
		label = onOff(settings.Paused, "恢复", "暂停")
	}
	groupID := a.selectedGroup
	a.async(label, func() error {
		_, err := manager.SaveCardSettings(context.Background(), groupID, prefix, autoRename, paused, updateExisting)
		return err
	})
}

func (a *uiApp) hasNumberedMembers() bool {
	members, _ := a.database.ListMembers(context.Background(), a.accountID, a.selectedGroup)
	for _, member := range members {
		if member.CardSuffix != "" {
			return true
		}
	}
	return false
}

func (a *uiApp) prefixChangeChoice(oldPrefix string) (updateExisting, accepted bool) {
	menu, _, _ := procCreatePopupMenu.Call()
	if menu == 0 {
		return false, false
	}
	defer procDestroyMenu.Call(menu)
	prompt := fmt.Sprintf("原本使用「%s」前缀的已改用户是否一同更改？", firstNonEmpty(oldPrefix, "DH"))
	procAppendMenuW.Call(menu, 0x0003, 0, uintptr(unsafe.Pointer(utf16(prompt))))
	procAppendMenuW.Call(menu, 0x0800, 0, 0)
	procAppendMenuW.Call(menu, 0, 31001, uintptr(unsafe.Pointer(utf16("确认"))))
	procAppendMenuW.Call(menu, 0, 31002, uintptr(unsafe.Pointer(utf16("保留"))))
	var cursor point
	procGetCursorPos.Call(uintptr(unsafe.Pointer(&cursor)))
	command, _, _ := procTrackPopupMenu.Call(menu, 0x0180, uintptr(cursor.X), uintptr(cursor.Y), 0, a.hwnd, 0)
	switch command {
	case 31001:
		return true, true
	case 31002:
		return false, true
	default:
		return false, false
	}
}

func (a *uiApp) previewCards() {
	manager := a.currentManager()
	if manager == nil || a.selectedGroup == 0 {
		a.setResult("请先选择群并连接旺商聊")
		return
	}
	groupID := a.selectedGroup
	a.async("生成建议", func() error {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		if err := manager.SyncMembers(ctx, groupID); err != nil {
			return err
		}
		preview, err := manager.PreviewCardNames(ctx, groupID)
		if err != nil {
			return err
		}
		a.mu.Lock()
		a.cardPreviewGroup = groupID
		a.cardPreview = make(map[int64]groupmgr.CardPlan, len(preview.Items))
		a.cardOverrides = make(map[int64]string)
		for _, item := range preview.Items {
			a.cardPreview[item.Member.UserID] = item
		}
		a.detail = fmt.Sprintf("将修改 %d，已规范 %d，角色排除 %d，身份缺失 %d，覆盖缺口 %d", preview.WillRename, preview.AlreadyManaged, preview.Excluded, preview.MissingIdentity, preview.CoverageGap)
		a.mu.Unlock()
		return nil
	})
}

func (a *uiApp) applyCardSuggestion() {
	if a.selectedGroup == 0 || a.selectedMember == 0 {
		a.setResult("请先选择成员")
		return
	}
	value := strings.TrimSpace(a.getEdit(editCardSuggestion))
	a.startAction("修改建议")
	a.mu.Lock()
	item, exists := a.cardPreview[a.selectedMember]
	if a.cardPreviewGroup != a.selectedGroup || !exists {
		a.mu.Unlock()
		a.setResult("请先预览群名片")
		return
	}
	if item.Suffix != "" {
		a.mu.Unlock()
		a.setResult("固定四位后缀为只读")
		return
	}
	if len([]rune(value)) != 2 {
		a.mu.Unlock()
		a.setResult("简称需要两个字符")
		return
	}
	item.SuggestedName = value
	a.cardPreview[a.selectedMember] = item
	a.cardOverrides[a.selectedMember] = value
	a.mu.Unlock()
	a.listSignatures[listMembers] = ""
	a.refreshLists()
	a.setResult("建议已更新")
}

func (a *uiApp) queueCards() {
	manager := a.currentManager()
	if manager == nil || a.selectedGroup == 0 {
		a.setResult("请先选择群并连接旺商聊")
		return
	}
	groupID := a.selectedGroup
	a.mu.RLock()
	overrides := make(map[int64]string)
	if a.cardPreviewGroup == groupID {
		for userID, value := range a.cardOverrides {
			overrides[userID] = value
		}
	}
	a.mu.RUnlock()
	a.async("一键执行", func() error {
		queued, err := manager.QueueCardPreview(context.Background(), groupID, overrides)
		if err == nil {
			a.mu.Lock()
			a.detail = fmt.Sprintf("已排队 %d 个群名片任务", queued)
			a.mu.Unlock()
		}
		return err
	})
}

func (a *uiApp) retryCards() {
	manager := a.currentManager()
	if manager == nil || a.selectedGroup == 0 {
		a.setResult("请先选择群并连接旺商聊")
		return
	}
	groupID := a.selectedGroup
	a.async("重试失败", func() error { return manager.RetryFailedCardJobs(context.Background(), groupID) })
}

func (a *uiApp) readCardNotices() {
	manager := a.currentManager()
	if manager == nil || a.selectedGroup == 0 {
		return
	}
	groupID := a.selectedGroup
	stats, _ := manager.CardStats(context.Background(), groupID)
	label := fmt.Sprintf("新增%d", stats.UnreadNew)
	a.async(label, func() error { return manager.MarkCardNoticesRead(context.Background(), groupID) })
}

func (a *uiApp) connectNow() {
	label := "测试连接"
	if a.page != pageSettings {
		status, _, online, _ := a.readStatus()
		label = status
		if online {
			label = "已连接"
		}
	}
	a.startAction(label)
	a.mu.Lock()
	a.lastConnect = time.Time{}
	a.mu.Unlock()
	go a.tryConnect()
}

func (a *uiApp) toggleGroup(kind string) {
	group, err := a.database.GetGroup(context.Background(), a.accountID, a.selectedGroup)
	if err != nil {
		a.setResult("请先选择群")
		return
	}
	manager := a.currentManager()
	if manager == nil {
		a.setResult("旺商聊尚未连接")
		return
	}
	if allowed, _, _ := manager.CanManageGroup(context.Background(), group.GroupID); !allowed {
		a.report(appcore.ErrManagerRequired)
		return
	}
	labels := map[string]string{"enabled": "启用 / 停用", "ai": "AI 开关", "moderation": "规则开关", "manual": "人工接管", "tasks": "任务提醒"}
	a.startAction(labels[kind])
	if kind == "enabled" {
		a.report(manager.SetGroupEnabled(context.Background(), group.GroupID, !group.Enabled))
		return
	}
	switch kind {
	case "ai":
		group.AIEnabled = !group.AIEnabled
	case "moderation":
		group.ModerationEnabled = !group.ModerationEnabled
	case "manual":
		group.ManualTakeover = !group.ManualTakeover
	case "tasks":
		group.TaskReminders = !group.TaskReminders
	}
	group.UpdatedAt = time.Now().UTC()
	if err := a.database.UpsertGroup(context.Background(), group); err != nil {
		a.setResult(userFacingError(err))
		return
	}
	a.report(nil)
}

func (a *uiApp) saveWelcome() {
	group, err := a.database.GetGroup(context.Background(), a.accountID, a.selectedGroup)
	if err != nil {
		a.setResult("请先选择群")
		return
	}
	if err := a.requireManagerGroups(context.Background(), []int64{group.GroupID}); err != nil {
		a.report(err)
		return
	}
	a.startAction("保存群设置")
	group.WelcomeMessage = strings.TrimSpace(a.getEdit(editWelcome))
	group.SummarySchedule = a.getSummarySchedule()
	group.UpdatedAt = time.Now().UTC()
	a.report(a.database.UpsertGroup(context.Background(), group))
}

func (a *uiApp) memberAction(kind string) {
	manager := a.currentManager()
	if manager == nil || a.selectedGroup == 0 || a.selectedMember == 0 {
		a.setResult("请先选择成员")
		return
	}
	groupID, userID := a.selectedGroup, a.selectedMember
	minutes := int64(10)
	if kind == "mute" {
		var err error
		minutes, err = strconv.ParseInt(strings.TrimSpace(a.getEdit(editMuteMinutes)), 10, 64)
		if err != nil || minutes < 1 || minutes > 43200 {
			a.setResult("禁言时长请填写 1 到 43200 分钟")
			return
		}
	}
	labels := map[string]string{"mute": "禁言", "unmute": "解禁", "rename": "恢复名片", "remove": "移出"}
	a.async(labels[kind], func() error {
		ctx, cancel := context.WithTimeout(context.Background(), 12*time.Second)
		defer cancel()
		switch kind {
		case "mute":
			return manager.MuteMember(ctx, groupID, userID, time.Duration(minutes)*time.Minute)
		case "unmute":
			return manager.UnmuteMember(ctx, groupID, userID)
		case "remove":
			return manager.RemoveMember(ctx, groupID, userID)
		case "rename":
			member, err := a.database.GetMember(ctx, a.accountID, groupID, userID)
			if err != nil {
				return err
			}
			return manager.RenameMember(ctx, groupID, userID, firstNonEmpty(member.LockedCardName, member.CardName))
		}
		return nil
	})
}

func (a *uiApp) cleanupInactiveMembers() {
	manager := a.currentManager()
	if manager == nil || a.selectedGroup == 0 {
		a.setResult("请先选择群并连接旺商聊")
		return
	}
	groupID := a.selectedGroup
	a.async("清理封禁成员", func() error {
		previewCtx, previewCancel := context.WithTimeout(context.Background(), 20*time.Second)
		candidates, err := manager.InactiveMembers(previewCtx, groupID)
		previewCancel()
		if err != nil {
			return err
		}
		if len(candidates) == 0 {
			a.setResult("本群没有需要清理的注销或封禁普通成员")
			return nil
		}
		message := fmt.Sprintf("发现 %d 个普通成员的账号状态或名称属于“该用户已注销”“已封禁用户”。\n\n确认将这些成员移出当前群？", len(candidates))
		answer, _, _ := procMessageBoxW.Call(a.hwnd, uintptr(unsafe.Pointer(utf16(message))), uintptr(unsafe.Pointer(utf16("清理封禁成员"))), mbYesNo|mbIconWarning)
		if answer != idYes {
			a.setResult("已取消清理")
			return nil
		}
		userIDs := make([]int64, 0, len(candidates))
		for _, member := range candidates {
			userIDs = append(userIDs, member.UserID)
		}
		cleanupCtx, cleanupCancel := context.WithTimeout(context.Background(), 60*time.Second)
		result, err := manager.CleanupInactiveMembers(cleanupCtx, groupID, userIDs)
		cleanupCancel()
		if err != nil {
			return err
		}
		summary := fmt.Sprintf("清理完成：发现 %d，已移出 %d，失败 %d，跳过 %d", len(candidates), result.Removed, result.Failed, result.Skipped)
		if result.Failed > 0 {
			return fmt.Errorf("%s；%s", summary, strings.Join(result.Errors, "；"))
		}
		a.setResult(summary)
		return nil
	})
}

func (a *uiApp) restoreMemberNames() {
	manager := a.currentManager()
	if manager == nil || a.selectedGroup == 0 {
		a.setResult("请先选择群并连接旺商聊")
		return
	}
	groupID := a.selectedGroup
	userIDs := a.checkedMemberIDs()
	count := len(userIDs)
	if count == 0 {
		members, _ := a.database.ListMembers(context.Background(), a.accountID, groupID)
		selfID, _ := strconv.ParseInt(a.accountID, 10, 64)
		for _, member := range members {
			original := strings.TrimSpace(member.OriginalCardName)
			if member.Present && member.UserID != selfID && member.Role == groupmgr.RoleMember && original != "" && (member.CardName != original || member.ManagedCardName != "") {
				count++
			}
		}
	}
	if count == 0 {
		a.setResult("当前群没有可恢复的普通成员名称")
		return
	}
	message := fmt.Sprintf("将 %d 个普通成员恢复为 DH 改名前保存的原名称。\n\n确认继续？", count)
	answer, _, _ := procMessageBoxW.Call(a.hwnd, uintptr(unsafe.Pointer(utf16(message))), uintptr(unsafe.Pointer(utf16("恢复群员名称"))), mbYesNo|mbIconWarning)
	if answer != idYes {
		a.setResult("已取消恢复")
		return
	}
	clear(a.selectedMembers)
	a.listSignatures[listMembers] = ""
	a.async("恢复群员名称", func() error {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Minute)
		defer cancel()
		result, err := manager.RestoreOriginalCardNames(ctx, groupID, userIDs)
		if err != nil {
			return err
		}
		summary := fmt.Sprintf("恢复完成：匹配 %d，成功 %d，失败 %d，跳过 %d", result.Matched, result.Restored, result.Failed, result.Skipped)
		if result.Failed > 0 {
			return fmt.Errorf("%s；%s", summary, strings.Join(result.Errors, "；"))
		}
		a.setResult(summary)
		return nil
	})
}

func (a *uiApp) groupMute(muted bool) {
	manager := a.currentManager()
	if manager == nil || a.selectedGroup == 0 {
		a.setResult("请先选择群")
		return
	}
	groupID := a.selectedGroup
	label := onOff(muted, "全员禁言", "解除全禁")
	a.async(label, func() error {
		ctx, cancel := context.WithTimeout(context.Background(), 12*time.Second)
		defer cancel()
		return manager.SetGroupMute(ctx, groupID, muted)
	})
}

func (a *uiApp) toggleGlobalPause() {
	if a.selectedGroup == 0 {
		a.setResult("请先选择群")
		return
	}
	if err := a.requireManagerGroups(context.Background(), []int64{a.selectedGroup}); err != nil {
		a.report(err)
		return
	}
	paused := a.setting("automation.paused", "false") != "true"
	a.startAction(onOff(paused, "暂停全部自动化", "恢复全部自动化"))
	err := a.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: "automation.paused", Value: strconv.FormatBool(paused), UpdatedAt: time.Now().UTC()})
	if err != nil {
		a.report(err)
		return
	}
	if manager := a.currentManager(); manager != nil {
		manager.SetPaused(paused)
	}
	a.setResult(onOff(paused, "全部自动化已暂停", "全部自动化已恢复"))
}

func (a *uiApp) testAI() {
	a.openAITestDialog()
}

func (a *uiApp) openAITestDialog() {
	if activeAITestDialog != nil && activeAITestDialog.hwnd != 0 {
		procShowWindow.Call(activeAITestDialog.hwnd, swRestore)
		procSetForegroundWindow.Call(activeAITestDialog.hwnd)
		return
	}
	instance, _, _ := procGetModuleHandleW.Call(0)
	className := utf16("DHNativeAITestWindow")
	wc := wndClassEx{Size: uint32(unsafe.Sizeof(wndClassEx{})), WndProc: syscall.NewCallback(aiTestWndProc), Instance: instance, Cursor: procLoadCursorWValue(), ClassName: className}
	procRegisterClassExW.Call(uintptr(unsafe.Pointer(&wc)))
	dialog := &aiTestDialog{app: a}
	hwnd, _, _ := procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(className)), uintptr(unsafe.Pointer(utf16("DH BOT AI 本地测试"))), wsOverlapped|wsCaption|wsSysMenu|wsThickFrame|wsMinimizeBox|wsVisible, 220, 120, 760, 620, 0, 0, instance, 0)
	if hwnd == 0 {
		a.setResult("AI 测试窗口启动失败")
		return
	}
	dialog.hwnd = hwnd
	activeAITestDialog = dialog
	createAITestControls(dialog)
	var client rect
	procGetClientRect.Call(hwnd, uintptr(unsafe.Pointer(&client)))
	layoutAITestControls(dialog, client.Right-client.Left, client.Bottom-client.Top)
	procShowWindow.Call(hwnd, swShow)
	procSetForegroundWindow.Call(hwnd)
	setWindowText(dialog.status, "仅本地测试，不读取群消息、不发送群消息")
}

func procLoadCursorWValue() uintptr {
	cursor, _, _ := procLoadCursorW.Call(0, idcArrow)
	return cursor
}

func createAITestControls(dialog *aiTestDialog) {
	font := dialog.app.fontBody
	editStyle := uintptr(wsChild | wsVisible | wsBorder | wsTabStop | esMultiLine | esAutoVScroll | esWantReturn)
	responseStyle := editStyle | esReadOnly
	dialog.promptLabel, _, _ = procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("STATIC"))), uintptr(unsafe.Pointer(utf16("测试问题"))), wsChild|wsVisible, 0, 0, 100, 24, dialog.hwnd, 104, 0, 0)
	dialog.prompt, _, _ = procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("EDIT"))), uintptr(unsafe.Pointer(utf16("你好，你能帮群管理员做什么？"))), editStyle, 0, 0, 100, 100, dialog.hwnd, 101, 0, 0)
	dialog.responseLabel, _, _ = procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("STATIC"))), uintptr(unsafe.Pointer(utf16("模型回复与调试信息"))), wsChild|wsVisible, 0, 0, 100, 24, dialog.hwnd, 105, 0, 0)
	dialog.response, _, _ = procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("EDIT"))), 0, responseStyle, 0, 0, 100, 100, dialog.hwnd, 102, 0, 0)
	dialog.status, _, _ = procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("STATIC"))), 0, wsChild|wsVisible, 0, 0, 100, 24, dialog.hwnd, 103, 0, 0)
	dialog.send, _, _ = procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("BUTTON"))), uintptr(unsafe.Pointer(utf16("发送测试"))), wsChild|wsVisible|wsTabStop, 0, 0, 110, 34, dialog.hwnd, 1, 0, 0)
	dialog.clear, _, _ = procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("BUTTON"))), uintptr(unsafe.Pointer(utf16("清空对话"))), wsChild|wsVisible|wsTabStop, 0, 0, 110, 34, dialog.hwnd, 2, 0, 0)
	dialog.close, _, _ = procCreateWindowExW.Call(0, uintptr(unsafe.Pointer(utf16("BUTTON"))), uintptr(unsafe.Pointer(utf16("关闭"))), wsChild|wsVisible|wsTabStop, 0, 0, 110, 34, dialog.hwnd, 3, 0, 0)
	for _, control := range []uintptr{dialog.promptLabel, dialog.prompt, dialog.responseLabel, dialog.response, dialog.status, dialog.send, dialog.clear, dialog.close} {
		if control != 0 {
			procSendMessageW.Call(control, wmSetFont, font, 1)
		}
	}
}

func layoutAITestControls(dialog *aiTestDialog, width, height int32) {
	if dialog == nil || dialog.hwnd == 0 {
		return
	}
	margin := int32(24)
	inner := width - margin*2
	procSetWindowPos.Call(dialog.promptLabel, 0, uintptr(margin), 16, uintptr(inner), 22, 0x0154)
	procSetWindowPos.Call(dialog.prompt, 0, uintptr(margin), 42, uintptr(inner), 100, 0x0154)
	procSetWindowPos.Call(dialog.responseLabel, 0, uintptr(margin), 156, uintptr(inner), 22, 0x0154)
	buttonY := max(height-76, 250)
	statusY := buttonY - 34
	responseHeight := max(statusY-198, 100)
	procSetWindowPos.Call(dialog.response, 0, uintptr(margin), 184, uintptr(inner), uintptr(responseHeight), 0x0154)
	procSetWindowPos.Call(dialog.status, 0, uintptr(margin), uintptr(statusY), uintptr(inner), 24, 0x0154)
	procSetWindowPos.Call(dialog.send, 0, uintptr(margin), uintptr(buttonY), 110, 34, 0x0154)
	procSetWindowPos.Call(dialog.clear, 0, uintptr(margin+122), uintptr(buttonY), 110, 34, 0x0154)
	procSetWindowPos.Call(dialog.close, 0, uintptr(width-134), uintptr(buttonY), 110, 34, 0x0154)
}

func (a *uiApp) sendAITest(dialog *aiTestDialog) {
	if dialog == nil {
		return
	}
	prompt := strings.TrimSpace(windowTextValue(dialog.prompt))
	if prompt == "" {
		setWindowText(dialog.status, "请输入测试问题")
		return
	}
	kind := strings.ToLower(strings.TrimSpace(a.getEdit(editAIKind)))
	base := strings.TrimSpace(a.getEdit(editAIBase))
	model := strings.TrimSpace(a.getEdit(editAIModel))
	webhookURL := strings.TrimSpace(a.getEdit(editWebhookURL))
	values, _ := a.secretStore.Load()
	key := strings.TrimSpace(a.getEdit(editAIKey))
	if key == secretPlaceholder {
		key = values["ai.default.apiKey"]
	}
	var provider aiprovider.Provider
	var providerErr error
	if kind == "webhook" {
		if webhookURL == "" {
			providerErr = errors.New("Webhook URL 为空")
		} else {
			provider = aiprovider.NewWebhook(aiprovider.WebhookConfig{URL: webhookURL, Token: values["webhook.token"], Timeout: 20 * time.Second})
		}
	} else {
		if base == "" || model == "" {
			providerErr = errors.New("请填写 AI Base URL 和模型")
		} else {
			provider = aiprovider.NewOpenAI(aiprovider.OpenAIConfig{BaseURL: base, Model: model, APIKey: key, Timeout: 30 * time.Second})
		}
	}
	if providerErr != nil {
		setWindowText(dialog.status, localizeUserText(providerErr.Error()))
		return
	}
	dialog.mu.Lock()
	if dialog.busy {
		dialog.mu.Unlock()
		return
	}
	dialog.busy = true
	history := append([]aiprovider.ContextMessage(nil), dialog.history...)
	dialog.mu.Unlock()
	setWindowText(dialog.status, "正在请求模型，测试结果不会发送到群里…")
	a.launch("AI 本地测试", func() {
		ctx, cancel := context.WithTimeout(context.Background(), 35*time.Second)
		defer cancel()
		started := time.Now()
		request := aiprovider.Request{Version: aiprovider.ContractVersion, EventID: fmt.Sprintf("local-test-%d", time.Now().UnixNano()), Persona: aiprovider.DefaultPersona, Group: aiprovider.Group{ID: 1, Name: "本地测试"}, Member: aiprovider.Member{UserID: 1, Name: "测试用户", Role: "member"}, Message: aiprovider.Message{Format: "test", Text: prompt, Time: time.Now().UnixMilli()}, RecentContext: history}
		decision, err := provider.Decide(ctx, request)
		reply := ""
		status := fmt.Sprintf("耗时 %d ms", time.Since(started).Milliseconds())
		if err != nil {
			status = "测试失败：" + userFacingError(err)
		} else {
			plainReply := firstNonEmpty(decision.Reply, "模型未返回文字回复")
			details := []string{plainReply}
			if strings.TrimSpace(decision.Reason) != "" {
				details = append(details, "理由："+strings.TrimSpace(decision.Reason))
			}
			if len(decision.Actions) > 0 || len(decision.Tasks) > 0 {
				details = append(details, fmt.Sprintf("测试返回：%d 个动作，%d 个任务（只展示）", len(decision.Actions), len(decision.Tasks)))
			}
			reply = strings.Join(details, "\r\n\r\n")
			modelLabel := onOff(kind == "webhook", "webhook", firstNonEmpty(model, "未填写模型"))
			status = fmt.Sprintf("测试完成 · 模型 %s · %d ms · 置信度 %.2f", modelLabel, time.Since(started).Milliseconds(), decision.Confidence)
			history = append(history, aiprovider.ContextMessage{UserID: 1, Name: "测试用户", Text: prompt, Time: time.Now().UnixMilli()}, aiprovider.ContextMessage{UserID: 1, Name: "DH", Text: reply, Time: time.Now().UnixMilli()})
			if len(history) > 20 {
				history = history[len(history)-20:]
			}
		}
		dialog.mu.Lock()
		dialog.history = history
		dialog.pendingReply, dialog.pendingStatus, dialog.busy = reply, status, false
		dialog.mu.Unlock()
		procPostMessageW.Call(dialog.hwnd, wmAITestResult, 0, 0)
	})
}

func aiTestWndProc(hwnd, message, wParam, lParam uintptr) uintptr {
	dialog := activeAITestDialog
	if dialog == nil || dialog.hwnd != hwnd {
		result, _, _ := procDefWindowProcW.Call(hwnd, message, wParam, lParam)
		return result
	}
	switch message {
	case wmSize:
		layoutAITestControls(dialog, int32(uint16(lParam&0xffff)), int32(uint16((lParam>>16)&0xffff)))
		return 0
	case wmCommand:
		switch int(wParam & 0xffff) {
		case 1:
			dialog.app.sendAITest(dialog)
		case 2:
			dialog.mu.Lock()
			dialog.history = nil
			dialog.mu.Unlock()
			setWindowText(dialog.response, "")
			setWindowText(dialog.status, "对话已清空")
		case 3:
			procDestroyWindow.Call(hwnd)
		}
		return 0
	case wmAITestResult:
		dialog.mu.Lock()
		reply, status := dialog.pendingReply, dialog.pendingStatus
		dialog.mu.Unlock()
		setWindowText(dialog.response, reply)
		setWindowText(dialog.status, status)
		return 0
	case wmClose:
		procDestroyWindow.Call(hwnd)
		return 0
	case wmDestroy:
		activeAITestDialog = nil
		return 0
	}
	result, _, _ := procDefWindowProcW.Call(hwnd, message, wParam, lParam)
	return result
}

func windowTextValue(hwnd uintptr) string {
	if hwnd == 0 {
		return ""
	}
	length, _, _ := procGetWindowTextLenW.Call(hwnd)
	buffer := make([]uint16, length+1)
	procGetWindowTextW.Call(hwnd, uintptr(unsafe.Pointer(&buffer[0])), length+1)
	return syscall.UTF16ToString(buffer)
}

func setWindowText(hwnd uintptr, value string) {
	if hwnd != 0 {
		procSetWindowTextW.Call(hwnd, uintptr(unsafe.Pointer(utf16(value))))
	}
}

func (a *uiApp) sendManual() {
	manager := a.currentManager()
	if manager == nil {
		a.setResult("旺商聊尚未连接")
		return
	}
	groupIDs := a.selectedGroupIDs(pageMessages)
	text := strings.TrimSpace(a.getEdit(editMessageText))
	if len(groupIDs) == 0 || text == "" {
		a.setResult("请选择群并填写消息")
		return
	}
	a.async("发送消息", func() error {
		if err := a.requireManagerGroups(context.Background(), groupIDs); err != nil {
			return err
		}
		for _, groupID := range groupIDs {
			if err := manager.SendManual(context.Background(), groupID, text); err != nil {
				return fmt.Errorf("%s: %w", firstNonEmpty(a.groupNames()[groupID], idText(groupID)), err)
			}
		}
		return nil
	})
}

func (a *uiApp) addRule() {
	groupIDs := a.selectedGroupIDs(pageRules)
	pattern := strings.TrimSpace(a.getEdit(editRulePattern))
	if len(groupIDs) == 0 || pattern == "" {
		a.setResult("请选择群并填写关键词")
		return
	}
	actions := make([]groupmgr.RuleAction, 0, 2)
	if a.isChecked(a.ruleRecall) {
		actions = append(actions, groupmgr.RuleAction{Type: groupmgr.ActionRecall})
	}
	if a.isChecked(a.ruleMute) {
		actions = append(actions, groupmgr.RuleAction{Type: groupmgr.ActionMute, Duration: 10 * time.Minute})
	}
	if len(actions) == 0 {
		a.setResult("请至少勾选撤回或禁言")
		return
	}
	if err := a.requireManagerGroups(context.Background(), groupIDs); err != nil {
		a.report(err)
		return
	}
	a.startAction("添加规则")
	rule := groupmgr.ModerationRule{GroupID: 0, Name: "关键词：" + pattern, Matcher: groupmgr.MatcherContains, Pattern: pattern, Priority: 150, Mode: groupmgr.RuleAuto, Enabled: true, ExemptRoles: []groupmgr.MemberRole{groupmgr.RoleOwner, groupmgr.RoleAdmin}, Cooldown: time.Minute, Actions: actions, CreatedAt: time.Now().UTC(), UpdatedAt: time.Now().UTC()}
	a.report(a.database.UpsertRule(context.Background(), &rule))
}

func (a *uiApp) manageRule(kind string) {
	if a.selectedRule == 0 {
		a.setResult("请先选择规则")
		return
	}
	for _, groupID := range a.selectedGroupIDs(pageRules) {
		rules, err := a.database.ListRules(context.Background(), groupID)
		if err != nil {
			a.report(err)
			return
		}
		for index := range rules {
			if rules[index].ID != a.selectedRule {
				continue
			}
			permissionGroup := rules[index].GroupID
			if permissionGroup == 0 {
				permissionGroup = groupID
			}
			if err := a.requireManagerGroups(context.Background(), []int64{permissionGroup}); err != nil {
				a.report(err)
				return
			}
			labels := map[string]string{"enabled": "启用 / 停用", "mode": "切换模式", "delete": "删除所选"}
			a.startAction(labels[kind])
			if kind == "delete" {
				err := a.database.DeleteRule(context.Background(), rules[index].ID)
				a.selectedRule = 0
				a.report(err)
				return
			}
			if kind == "enabled" {
				rules[index].Enabled = !rules[index].Enabled
			} else {
				switch rules[index].Mode {
				case groupmgr.RuleAuto:
					rules[index].Mode = groupmgr.RuleObserve
				case groupmgr.RuleObserve:
					rules[index].Mode = groupmgr.RuleDryRun
				default:
					rules[index].Mode = groupmgr.RuleAuto
				}
			}
			rules[index].UpdatedAt = time.Now().UTC()
			a.report(a.database.UpsertRule(context.Background(), &rules[index]))
			return
		}
	}
	a.setResult("所选规则已不存在")
}

func (a *uiApp) exportRules() {
	groupIDs := a.selectedGroupIDs(pageRules)
	if err := a.requireManagerGroups(context.Background(), groupIDs); err != nil {
		a.report(err)
		return
	}
	a.startAction("导出")
	var rules []groupmgr.ModerationRule
	seen := make(map[int64]bool)
	for _, groupID := range groupIDs {
		items, err := a.database.ListRules(context.Background(), groupID)
		if err != nil {
			a.report(err)
			return
		}
		for _, item := range items {
			if !seen[item.ID] {
				seen[item.ID] = true
				rules = append(rules, item)
			}
		}
	}
	raw, err := moderation.ExportRules(moderation.RulesFromModel(rules))
	if err == nil {
		err = os.WriteFile(a.getEdit(editRulePath), raw, 0o600)
	}
	a.report(err)
}

func (a *uiApp) importRules() {
	groupIDs := a.selectedGroupIDs(pageRules)
	if err := a.requireManagerGroups(context.Background(), groupIDs); err != nil {
		a.report(err)
		return
	}
	a.startAction("导入")
	raw, err := os.ReadFile(a.getEdit(editRulePath))
	if err != nil {
		a.report(err)
		return
	}
	rules, err := moderation.ImportRules(raw)
	if err != nil {
		a.report(err)
		return
	}
	for _, rule := range rules {
		rule.ID, rule.GroupID = 0, 0
		model := modelRule(rule)
		if err = a.database.UpsertRule(context.Background(), &model); err != nil {
			break
		}
	}
	a.report(err)
}

func modelRule(rule moderation.ModerationRule) groupmgr.ModerationRule {
	actions := make([]groupmgr.RuleAction, 0, len(rule.Actions))
	for _, action := range rule.Actions {
		message := firstNonEmpty(action.Message, action.Reply)
		actions = append(actions, groupmgr.RuleAction{Type: groupmgr.ActionType(action.Type), Duration: action.Duration, Message: message})
	}
	roles := make([]groupmgr.MemberRole, len(rule.ExemptRoles))
	for index, role := range rule.ExemptRoles {
		roles[index] = groupmgr.MemberRole(role)
	}
	return groupmgr.ModerationRule{GroupID: rule.GroupID, Name: rule.Name, Matcher: groupmgr.MatcherType(rule.Matcher), Pattern: rule.Pattern, Threshold: rule.Threshold, Count: rule.Count, Window: rule.Window, Cooldown: rule.Cooldown, Priority: rule.Priority, Mode: groupmgr.RuleMode(rule.Mode), Enabled: rule.Enabled, SemanticThreshold: rule.SemanticThreshold, ExemptRoles: roles, ExemptUserIDs: rule.ExemptUserIDs, Actions: actions, CreatedAt: time.Now().UTC(), UpdatedAt: time.Now().UTC()}
}

func (a *uiApp) saveKnowledge() {
	if a.selectedKnowledgeBase == 0 {
		a.setResult("请选择知识库")
		return
	}
	title, content := strings.TrimSpace(a.getEdit(editKnowledgeTitle)), strings.TrimSpace(a.getEdit(editKnowledgeContent))
	if title == "" || content == "" {
		a.setResult("请选择知识库并填写标题和内容")
		return
	}
	a.startAction("保存内容")
	err := a.database.UpsertKnowledgeDocument(context.Background(), &groupmgr.KnowledgeDocument{BaseID: a.selectedKnowledgeBase, Title: title, Kind: "faq", Content: content, Source: "手工编辑"})
	a.report(err)
}

func (a *uiApp) importKnowledge() {
	if a.selectedKnowledgeBase == 0 {
		a.setResult("请选择知识库")
		return
	}
	a.startAction("导入文档")
	document, err := knowledge.LoadDocument(strings.TrimSpace(a.getEdit(editKnowledgePath)), 0)
	if err != nil {
		a.report(err)
		return
	}
	err = a.database.UpsertKnowledgeDocument(context.Background(), &groupmgr.KnowledgeDocument{BaseID: a.selectedKnowledgeBase, Title: document.Title, Kind: "document", Content: document.Text, Source: document.Source})
	a.report(err)
}

func (a *uiApp) knowledgeBaseLabel() string {
	for _, base := range a.knowledgeBases {
		if base.ID == a.selectedKnowledgeBase {
			return base.Name + "  ▾"
		}
	}
	return "请选择知识库  ▾"
}

func (a *uiApp) knowledgeAccountID() string {
	if strings.TrimSpace(a.accountID) != "" {
		return a.accountID
	}
	return localKnowledgeAccountID
}

func (a *uiApp) refreshKnowledgeBases() {
	accountID := a.knowledgeAccountID()
	_, _ = a.database.EnsureDefaultKnowledgeBase(context.Background(), accountID)
	bases, err := a.database.ListKnowledgeBases(context.Background(), accountID)
	if err == nil {
		a.knowledgeBases = bases
		if a.selectedKnowledgeBase == 0 && len(bases) > 0 {
			a.selectedKnowledgeBase = bases[0].ID
			if a.page == pageKnowledge {
				a.setKnowledgeEdits()
			}
		}
	}
}

func (a *uiApp) showKnowledgeBaseMenu() {
	a.refreshKnowledgeBases()
	if len(a.knowledgeBases) == 0 {
		a.setResult("当前账号还没有知识库")
		return
	}
	menu, _, _ := procCreatePopupMenu.Call()
	if menu == 0 {
		return
	}
	defer procDestroyMenu.Call(menu)
	const baseCommand = 16000
	for index, base := range a.knowledgeBases {
		procAppendMenuW.Call(menu, 0, baseCommand+uintptr(index), uintptr(unsafe.Pointer(utf16(base.Name))))
	}
	var cursor point
	procGetCursorPos.Call(uintptr(unsafe.Pointer(&cursor)))
	command, _, _ := procTrackPopupMenu.Call(menu, 0x0180, uintptr(cursor.X), uintptr(cursor.Y), 0, a.hwnd, 0)
	index := int(command - baseCommand)
	if index >= 0 && index < len(a.knowledgeBases) {
		a.selectedKnowledgeBase = a.knowledgeBases[index].ID
		a.setKnowledgeEdits()
		a.listSignatures[listKnowledge] = ""
		a.refreshLists()
		a.invalidate()
	}
}

func (a *uiApp) setKnowledgeEdits() {
	for _, base := range a.knowledgeBases {
		if base.ID == a.selectedKnowledgeBase {
			a.setEdit(editKnowledgeBase, base.Name)
			a.setEdit(editKnowledgeDesc, base.Description)
			return
		}
	}
}

func (a *uiApp) bindKnowledgeGroups(bind bool) {
	if a.selectedKnowledgeBase == 0 {
		a.setResult("请选择知识库")
		return
	}
	groupIDs := a.selectedGroupIDs(pageKnowledge)
	if len(groupIDs) == 0 {
		a.setResult("请选择要绑定的群")
		return
	}
	var err error
	accountID := a.knowledgeAccountID()
	if bind {
		err = a.database.BindKnowledgeBaseGroups(context.Background(), accountID, a.selectedKnowledgeBase, groupIDs)
	} else {
		err = a.database.UnbindKnowledgeBaseGroups(context.Background(), accountID, a.selectedKnowledgeBase, groupIDs)
	}
	a.report(err)
}

func (a *uiApp) createKnowledgeBase() {
	name := strings.TrimSpace(a.getEdit(editKnowledgeBase))
	if name == "" || name == "请选择知识库" {
		name = "新知识库"
	}
	base := groupmgr.KnowledgeBase{AccountID: a.knowledgeAccountID(), Name: name, Description: strings.TrimSpace(a.getEdit(editKnowledgeDesc)), Enabled: true}
	err := a.database.CreateKnowledgeBase(context.Background(), &base)
	if err == nil {
		a.selectedKnowledgeBase = base.ID
		a.refreshKnowledgeBases()
		a.setKnowledgeEdits()
	}
	a.report(err)
}

func (a *uiApp) copyKnowledgeBase() {
	if a.selectedKnowledgeBase == 0 {
		a.setResult("请选择知识库")
		return
	}
	var source *groupmgr.KnowledgeBase
	for index := range a.knowledgeBases {
		if a.knowledgeBases[index].ID == a.selectedKnowledgeBase {
			source = &a.knowledgeBases[index]
			break
		}
	}
	if source == nil {
		a.setResult("所选知识库不存在")
		return
	}
	accountID := a.knowledgeAccountID()
	copyBase := groupmgr.KnowledgeBase{AccountID: accountID, Name: source.Name + "（副本）", Description: source.Description, Enabled: true}
	if err := a.database.CreateKnowledgeBase(context.Background(), &copyBase); err != nil {
		a.report(err)
		return
	}
	docs, err := a.database.ListKnowledgeBasesDocuments(context.Background(), accountID, source.ID)
	if err == nil {
		for _, doc := range docs {
			doc.ID, doc.BaseID = 0, copyBase.ID
			_ = a.database.UpsertKnowledgeDocument(context.Background(), &doc)
		}
	}
	a.selectedKnowledgeBase = copyBase.ID
	a.refreshKnowledgeBases()
	a.setKnowledgeEdits()
	a.report(err)
}

func (a *uiApp) createTask() {
	groupIDs := a.selectedGroupIDs(pageTasks)
	assignee, _ := strconv.ParseInt(strings.TrimSpace(a.getEdit(editTaskAssignee)), 10, 64)
	task := groupmgr.Task{AssigneeID: assignee, Title: strings.TrimSpace(a.getEdit(editTaskTitle)), Status: groupmgr.TaskPending, CreatedAt: time.Now().UTC(), UpdatedAt: time.Now().UTC()}
	if value := strings.TrimSpace(a.getEdit(editTaskDue)); value != "" {
		due, err := parseUserTime(value)
		if err != nil {
			a.setResult("到期时间请填写 YYYY-MM-DD HH:MM")
			return
		}
		task.DueAt = &due
	}
	if len(groupIDs) == 0 || task.Title == "" {
		a.setResult("请选择群并填写任务")
		return
	}
	if err := a.requireManagerGroups(context.Background(), groupIDs); err != nil {
		a.report(err)
		return
	}
	a.startAction("创建任务")
	var err error
	for _, groupID := range groupIDs {
		item := task
		item.GroupID = groupID
		if err = a.database.UpsertTask(context.Background(), &item); err != nil {
			break
		}
	}
	a.report(err)
}

func (a *uiApp) completeTask() {
	if a.selectedTask == 0 {
		a.setResult("请先选择任务")
		return
	}
	for _, groupID := range a.selectedGroupIDs(pageTasks) {
		tasks, _ := a.database.ListTasks(context.Background(), groupID, "")
		for _, task := range tasks {
			if task.ID == a.selectedTask {
				if err := a.requireManagerGroups(context.Background(), []int64{task.GroupID}); err != nil {
					a.report(err)
					return
				}
				a.startAction("完成所选")
				task.Status, task.UpdatedAt = groupmgr.TaskCompleted, time.Now().UTC()
				a.report(a.database.UpsertTask(context.Background(), &task))
				return
			}
		}
	}
	a.setResult("所选任务已不存在")
}

func (a *uiApp) saveGroupSchedule() {
	groupIDs := a.selectedGroupIDs(pageTasks)
	if len(groupIDs) == 0 {
		a.setResult("请先选择需要定时开关的群")
		return
	}
	if err := a.requireManagerGroups(context.Background(), groupIDs); err != nil {
		a.report(err)
		return
	}
	name := strings.TrimSpace(a.getEdit(editScheduleName))
	openTime := strings.TrimSpace(a.getEdit(editScheduleOpen))
	closeTime := strings.TrimSpace(a.getEdit(editScheduleClose))
	schedule := groupmgr.GroupSchedule{AccountID: a.accountID, Name: name, OpenTime: openTime, CloseTime: closeTime, Timezone: time.Local.String(), Enabled: a.isChecked(a.scheduleEnabled), GroupIDs: groupIDs, UpdatedAt: time.Now().UTC()}
	if err := a.database.UpsertGroupSchedule(context.Background(), &schedule); err != nil {
		a.report(err)
		return
	}
	if err := a.database.BindScheduleGroups(context.Background(), a.accountID, schedule.ID, groupIDs); err != nil {
		if !strings.Contains(err.Error(), "群已绑定其他启用计划") {
			a.report(err)
			return
		}
		message := "所选群中已有群属于其他启用计划。\n\n是否把这些群转移到当前计划？"
		answer, _, _ := procMessageBoxW.Call(a.hwnd, uintptr(unsafe.Pointer(utf16(message))), uintptr(unsafe.Pointer(utf16("转移群计划"))), mbYesNo|mbIconWarning)
		if answer != idYes {
			a.setResult("已保留原群计划")
			return
		}
		if err := a.database.MoveScheduleGroups(context.Background(), a.accountID, schedule.ID, groupIDs); err != nil {
			a.report(err)
			return
		}
	}
	for key, value := range map[string]string{"schedule.name": name, "schedule.open": openTime, "schedule.close": closeTime, "schedule.enabled": onOff(schedule.Enabled, "true", "false")} {
		_ = a.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: key, Value: value, UpdatedAt: time.Now().UTC()})
	}
	a.setResult("群发言计划已保存")
}

func (a *uiApp) summarize() {
	manager := a.currentManager()
	if manager == nil {
		a.setResult("旺商聊尚未连接")
		return
	}
	groupIDs := a.selectedGroupIDs(pageTasks)
	if len(groupIDs) == 0 {
		a.setResult("请先选择群")
		return
	}
	a.async("生成群摘要", func() error {
		ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
		defer cancel()
		if err := a.requireManagerGroups(ctx, groupIDs); err != nil {
			return err
		}
		for _, groupID := range groupIDs {
			if _, err := manager.Summarize(ctx, groupID); err != nil {
				return err
			}
		}
		return nil
	})
}

func (a *uiApp) showSelectedSummary() {
	if a.selectedSummary == 0 {
		a.setResult("请先选择一条每日摘要")
		return
	}
	summaries, err := a.database.ListDailySummaries(context.Background(), a.accountID, 100)
	if err != nil {
		a.report(err)
		return
	}
	for _, summary := range summaries {
		if summary.ID != a.selectedSummary {
			continue
		}
		groupName := firstNonEmpty(a.groupNames()[summary.GroupID], "未知群")
		title := fmt.Sprintf("%s · %s", groupName, summary.CreatedAt.Local().Format("2006-01-02 15:04"))
		procMessageBoxW.Call(a.hwnd, uintptr(unsafe.Pointer(utf16(summary.Content))), uintptr(unsafe.Pointer(utf16(title))), 0)
		return
	}
	a.setResult("所选摘要已不存在")
}

func (a *uiApp) saveSettings() {
	settings := map[string]string{
		"wsl.path": a.getEdit(editWangPath), "wsl.auto_start": onOff(a.isChecked(a.autoStart), "true", "false"), "wsl.persist_login": onOff(a.isChecked(a.persistLogin), "true", "false"), "wsl.profile_id": defaultWangProfile,
		"bridge.devtools": a.getEdit(editDevTools), "bridge.endpoint": a.getEdit(editEndpoint),
		"ai.wake_words": "@DH", "ai.kind": strings.ToLower(strings.TrimSpace(a.getEdit(editAIKind))),
		"ai.base_url": a.getEdit(editAIBase), "ai.model": firstNonEmpty(a.getEdit(editAIModel), defaultAIModel), "ai.webhook_url": a.getEdit(editWebhookURL),
	}
	if err := urlguard.ValidateLoopback(settings["bridge.devtools"]); err != nil {
		a.setResult("旺商聊 DevTools：" + err.Error())
		return
	}
	if err := urlguard.ValidateLoopback(settings["bridge.endpoint"]); err != nil {
		a.setResult("内嵌桥地址：" + err.Error())
		return
	}
	if settings["ai.base_url"] != "" {
		if err := urlguard.ValidateOutbound(settings["ai.base_url"]); err != nil {
			a.setResult("AI Base URL：" + err.Error())
			return
		}
	}
	if settings["ai.webhook_url"] != "" {
		if err := urlguard.ValidateOutbound(settings["ai.webhook_url"]); err != nil {
			a.setResult("Webhook URL：" + err.Error())
			return
		}
	}
	a.startAction("保存设置")
	for key, value := range settings {
		if err := a.database.SetSetting(context.Background(), groupmgr.AppSetting{Key: key, Value: strings.TrimSpace(value), UpdatedAt: time.Now().UTC()}); err != nil {
			a.report(err)
			return
		}
	}
	values, _ := a.secretStore.Load()
	enteredKey := strings.TrimSpace(a.getEdit(editAIKey))
	if enteredKey != secretPlaceholder {
		values["ai.default.apiKey"] = enteredKey
		a.apiKeyStored = enteredKey != ""
	}
	if err := a.secretStore.Save(values); err != nil {
		a.report(err)
		return
	}
	a.mu.Lock()
	a.configureProviderLocked()
	a.mu.Unlock()
	a.startBridge()
	a.report(nil)
}

func (a *uiApp) configureProviderLocked() {
	if a.manager == nil {
		return
	}
	a.manager.WakeWords = []string{"@DH"}
	values, _ := a.secretStore.Load()
	switch a.setting("ai.kind", "openai") {
	case "webhook":
		url := a.setting("ai.webhook_url", "")
		if url == "" {
			a.manager.SetProvider(nil)
		} else {
			a.manager.SetProvider(aiprovider.NewWebhook(aiprovider.WebhookConfig{URL: url, Token: values["webhook.token"], Timeout: 15 * time.Second}))
		}
	default:
		base, model := a.setting("ai.base_url", defaultAIBaseURL), a.setting("ai.model", defaultAIModel)
		if base == "" || model == "" {
			a.manager.SetProvider(nil)
		} else {
			a.manager.SetProvider(aiprovider.NewOpenAI(aiprovider.OpenAIConfig{BaseURL: base, Model: model, APIKey: values["ai.default.apiKey"], Timeout: 15 * time.Second}))
		}
	}
}

func (a *uiApp) populateGroupEdits() {
	if a.selectedGroup == 0 {
		return
	}
	if group, err := a.database.GetGroup(context.Background(), a.accountID, a.selectedGroup); err == nil {
		a.setEdit(editWelcome, group.WelcomeMessage)
		a.setSummarySchedule(group.SummarySchedule)
	}
	if settings, err := a.database.GetCardSettings(context.Background(), a.accountID, a.selectedGroup); err == nil {
		a.setEdit(editCardPrefix, settings.Prefix)
	}
	a.populateMemberSuggestion()
}

func (a *uiApp) populateMemberSuggestion() {
	if a.selectedGroup == 0 || a.selectedMember == 0 {
		a.setEdit(editCardSuggestion, "")
		return
	}
	preview := a.cardPreviewSnapshot(a.selectedGroup)
	if item, exists := preview[a.selectedMember]; exists {
		a.setEdit(editCardSuggestion, item.SuggestedName)
		return
	}
	if member, err := a.database.GetMember(context.Background(), a.accountID, a.selectedGroup, a.selectedMember); err == nil {
		a.setEdit(editCardSuggestion, firstNonEmpty(member.ManagedCardName, member.CardName))
	}
}

func parseUserTime(value string) (time.Time, error) {
	parsed, err := time.ParseInLocation("2006-01-02 15:04", value, time.Local)
	if err == nil {
		return parsed, nil
	}
	short, shortErr := time.ParseInLocation("01-02 15:04", value, time.Local)
	if shortErr != nil {
		return time.Time{}, err
	}
	now := time.Now()
	return time.Date(now.Year(), short.Month(), short.Day(), short.Hour(), short.Minute(), 0, 0, time.Local), nil
}

func (a *uiApp) groups() []groupmgr.Group {
	groups, _ := a.database.ListGroups(context.Background(), a.accountID, false)
	return groups
}

func (a *uiApp) groupSchedules() []groupmgr.GroupSchedule {
	schedules, _ := a.database.ListGroupSchedules(context.Background(), a.accountID)
	return schedules
}

func scheduleNextAction(schedule groupmgr.GroupSchedule) string {
	if !schedule.Enabled {
		return "已暂停"
	}
	open, openErr := time.ParseInLocation("15:04", schedule.OpenTime, time.Local)
	close, closeErr := time.ParseInLocation("15:04", schedule.CloseTime, time.Local)
	if openErr != nil || closeErr != nil {
		return "时间未设置"
	}
	now := time.Now()
	openAt := time.Date(now.Year(), now.Month(), now.Day(), open.Hour(), open.Minute(), 0, 0, time.Local)
	closeAt := time.Date(now.Year(), now.Month(), now.Day(), close.Hour(), close.Minute(), 0, 0, time.Local)
	if !openAt.After(now) {
		openAt = openAt.AddDate(0, 0, 1)
	}
	if !closeAt.After(now) {
		closeAt = closeAt.AddDate(0, 0, 1)
	}
	if closeAt.Before(openAt) {
		return "关群 " + closeAt.Format("01-02 15:04")
	}
	return "开群 " + openAt.Format("01-02 15:04")
}

func (a *uiApp) groupNames() map[int64]string {
	names := make(map[int64]string)
	for _, group := range a.groups() {
		names[group.GroupID] = firstNonEmpty(group.Name, "未命名群")
	}
	return names
}

func (a *uiApp) selectedGroupIDs(page int) []int64 {
	selection, initialized := a.pageGroups[page]
	ids := make([]int64, 0, len(selection))
	for groupID, selected := range selection {
		if selected {
			ids = append(ids, groupID)
		}
	}
	if !initialized && a.selectedGroup != 0 {
		ids = append(ids, a.selectedGroup)
	}
	sort.Slice(ids, func(i, j int) bool { return ids[i] < ids[j] })
	return ids
}

func (a *uiApp) groupPickerLabel(page int) string {
	ids := a.selectedGroupIDs(page)
	if len(ids) == 0 {
		return "选择群  ▾"
	}
	if len(ids) == 1 {
		return firstNonEmpty(a.groupNames()[ids[0]], "选择群") + "  ▾"
	}
	return fmt.Sprintf("已选择 %d 个群  ▾", len(ids))
}

func (a *uiApp) showGroupMenu(page int) {
	groups := a.groups()
	if len(groups) == 0 {
		a.setResult("请先同步群")
		return
	}
	menu, _, _ := procCreatePopupMenu.Call()
	if menu == 0 {
		return
	}
	defer procDestroyMenu.Call(menu)
	const groupCommandBase = 10000
	selection := a.pageGroups[page]
	if selection == nil {
		selection = make(map[int64]bool)
		if a.selectedGroup != 0 {
			selection[a.selectedGroup] = true
		}
		a.pageGroups[page] = selection
	}
	for index, group := range groups {
		command := uintptr(groupCommandBase + index)
		procAppendMenuW.Call(menu, 0, command, uintptr(unsafe.Pointer(utf16(firstNonEmpty(group.Name, "未命名群")))))
		if selection[group.GroupID] {
			procCheckMenuItem.Call(menu, command, 0x0008)
		}
	}
	procAppendMenuW.Call(menu, 0x0800, 0, 0)
	procAppendMenuW.Call(menu, 0, 20001, uintptr(unsafe.Pointer(utf16("全选"))))
	procAppendMenuW.Call(menu, 0, 20002, uintptr(unsafe.Pointer(utf16("清空"))))
	var cursor point
	procGetCursorPos.Call(uintptr(unsafe.Pointer(&cursor)))
	command, _, _ := procTrackPopupMenu.Call(menu, 0x0180, uintptr(cursor.X), uintptr(cursor.Y), 0, a.hwnd, 0)
	switch command {
	case 20001:
		for _, group := range groups {
			selection[group.GroupID] = true
		}
	case 20002:
		clear(selection)
	default:
		index := int(command) - groupCommandBase
		if index >= 0 && index < len(groups) {
			groupID := groups[index].GroupID
			selection[groupID] = !selection[groupID]
		}
	}
	a.listSignatures[a.currentListID()] = ""
	if page == pageRules {
		if manager := a.currentManager(); manager != nil {
			for _, groupID := range a.selectedGroupIDs(pageRules) {
				if err := manager.EnsureDefaultRules(context.Background(), groupID); err != nil {
					a.setResult(userFacingError(err))
					break
				}
			}
		}
	}
	a.refreshLists()
	a.invalidate()
}

func (a *uiApp) selectedGroupSet(page int) map[int64]bool {
	set := make(map[int64]bool)
	for _, groupID := range a.selectedGroupIDs(page) {
		set[groupID] = true
	}
	return set
}

func (a *uiApp) requireManagerGroups(ctx context.Context, groupIDs []int64) error {
	manager := a.currentManager()
	if manager == nil {
		return fmt.Errorf("旺商聊尚未连接")
	}
	if len(groupIDs) == 0 {
		return fmt.Errorf("请先选择群")
	}
	for _, groupID := range groupIDs {
		allowed, _, _ := manager.CanManageGroup(ctx, groupID)
		if !allowed {
			return appcore.ErrManagerRequired
		}
	}
	return nil
}

func (a *uiApp) currentManager() *appcore.Manager {
	a.mu.RLock()
	defer a.mu.RUnlock()
	return a.manager
}

func (a *uiApp) setting(key, fallback string) string {
	value, err := a.database.GetSetting(context.Background(), key)
	if err == nil && value.Value != "" {
		return value.Value
	}
	return fallback
}

func (a *uiApp) getEditOrSetting(id int, key, fallback string) string {
	if control := a.edits[id]; control != 0 {
		if value := strings.TrimSpace(a.getEdit(id)); value != "" {
			return value
		}
	}
	return a.setting(key, fallback)
}

func (a *uiApp) startAction(label string) {
	a.mu.Lock()
	a.activeAction = label
	a.animationPhase = 0
	a.animationUntil = time.Now().Add(700 * time.Millisecond)
	a.mu.Unlock()
	a.invalidate()
}

func (a *uiApp) advanceAnimation() bool {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.activeAction == "" {
		return false
	}
	if a.busy || time.Now().Before(a.animationUntil) {
		a.animationPhase++
		return true
	}
	a.activeAction = ""
	return true
}

func (a *uiApp) animatedButtonLabel(label string) string {
	a.mu.RLock()
	defer a.mu.RUnlock()
	matched := a.activeAction == label
	if !matched && (a.activeAction == "暂停全部自动化" || a.activeAction == "恢复全部自动化") {
		matched = label == "暂停全部自动化" || label == "恢复全部自动化"
	}
	if !matched && a.activeAction == "恢复群员名称" {
		matched = strings.HasPrefix(label, "恢复所有群员名称") || strings.HasPrefix(label, "恢复该群员名称")
	}
	if !matched {
		return label
	}
	frames := []string{"▰▱▱", "▱▰▱", "▱▱▰", "▱▰▱"}
	return a.activeAction + "  " + frames[a.animationPhase%len(frames)]
}

func (a *uiApp) async(label string, work func() error) {
	a.mu.Lock()
	if a.busy {
		a.mu.Unlock()
		return
	}
	a.busy = true
	a.activeAction = label
	a.animationPhase = 0
	a.animationUntil = time.Now().Add(700 * time.Millisecond)
	a.detail = ""
	a.mu.Unlock()
	a.launch(label, func() {
		err := work()
		a.mu.Lock()
		a.busy = false
		if err != nil {
			a.status, a.detail = "操作失败", userFacingError(err)
		} else {
			a.status = "操作完成"
		}
		a.mu.Unlock()
		procPostMessageW.Call(a.hwnd, wmAppRefresh, 0, 0)
	})
}

func (a *uiApp) launch(label string, work func()) {
	a.workerWG.Add(1)
	go func() {
		defer a.workerWG.Done()
		defer func() {
			if recovered := recover(); recovered != nil {
				path := writeCrashLog(label+"异常", fmt.Sprint(recovered))
				a.mu.Lock()
				a.status = "操作失败"
				a.detail = "后台任务出现异常"
				a.mu.Unlock()
				if path != "" {
					a.mu.Lock()
					a.detail = "后台任务出现异常，诊断日志：" + path
					a.mu.Unlock()
				}
				procPostMessageW.Call(a.hwnd, wmAppRefresh, 0, 0)
			}
		}()
		work()
	}()
}

func (a *uiApp) setConnection(status, detail string, online bool) {
	a.mu.Lock()
	a.status, a.detail, a.online = status, detail, online
	a.mu.Unlock()
	procPostMessageW.Call(a.hwnd, wmAppRefresh, 0, 0)
}

func (a *uiApp) setResult(message string) {
	a.mu.Lock()
	a.detail = message
	a.mu.Unlock()
	procPostMessageW.Call(a.hwnd, wmAppRefresh, 0, 0)
}

func (a *uiApp) report(err error) {
	if err != nil {
		a.setResult(userFacingError(err))
	} else {
		a.setResult("操作完成")
	}
}

func userFacingError(err error) string {
	if errors.Is(err, groupmgr.ErrPermissionDenied) || errors.Is(err, appcore.ErrManagerRequired) {
		return appcore.ErrManagerRequired.Error()
	}
	lower := strings.ToLower(err.Error())
	if strings.Contains(lower, "127.0.0.1:9222") && (strings.Contains(lower, "connect") || strings.Contains(lower, "dial tcp") || strings.Contains(lower, "devtools")) {
		return "本机 9222 DevTools 尚未响应，请确认旺商聊已由 DH BOT 启动并进入登录界面"
	}
	if strings.Contains(lower, "127.0.0.1:51235") && (strings.Contains(lower, "connect") || strings.Contains(lower, "dial tcp")) {
		return "DH BOT 内嵌桥尚未响应，请退出重复运行的程序后重试"
	}
	if strings.Contains(lower, "nim") && (strings.Contains(lower, "未就绪") || strings.Contains(lower, "尚未初始化")) {
		return "NIM 尚未初始化，请先在旺商聊完成登录并等待会话就绪"
	}
	return localizeUserText(err.Error())
}

func (a *uiApp) readStatus() (string, string, bool, bool) {
	a.mu.RLock()
	defer a.mu.RUnlock()
	return a.status, a.detail, a.online, a.busy
}

func (a *uiApp) invalidate() {
	if a.hwnd != 0 {
		// Let Windows coalesce paints and leave double-buffered child controls
		// alone. Redrawing the full child tree on every poll caused visible flash.
		procInvalidateRect.Call(a.hwnd, 0, 0)
	}
}

func (a *uiApp) getEdit(id int) string {
	control := a.edits[id]
	if control == 0 {
		return ""
	}
	if id == editScheduleOpen || id == editScheduleClose {
		return a.getClockControl(control)
	}
	length, _, _ := procGetWindowTextLenW.Call(control)
	buffer := make([]uint16, length+1)
	procGetWindowTextW.Call(control, uintptr(unsafe.Pointer(&buffer[0])), length+1)
	return syscall.UTF16ToString(buffer)
}

func (a *uiApp) setEdit(id int, value string) {
	if control := a.edits[id]; control != 0 {
		if id == editScheduleOpen || id == editScheduleClose {
			a.setClockControl(control, value)
			return
		}
		procSetWindowTextW.Call(control, uintptr(unsafe.Pointer(utf16(value))))
	}
}

func (a *uiApp) getClockControl(control uintptr) string {
	if control == 0 {
		return ""
	}
	var value systemTime
	if result, _, _ := procSendMessageW.Call(control, dtmGetSystemTime, 0, uintptr(unsafe.Pointer(&value))); result != gdtValid {
		return ""
	}
	return fmt.Sprintf("%02d:%02d", value.Hour, value.Minute)
}

func (a *uiApp) setClockControl(control uintptr, clock string) {
	if control == 0 {
		return
	}
	parsed, err := time.Parse("15:04", strings.TrimSpace(clock))
	if err != nil {
		parsed, _ = time.Parse("15:04", "08:00")
	}
	value := clockSystemTime(parsed)
	procSendMessageW.Call(control, dtmSetSystemTime, gdtValid, uintptr(unsafe.Pointer(&value)))
}

func clockSystemTime(clock time.Time) systemTime {
	now := time.Now()
	return systemTime{
		Year: uint16(now.Year()), Month: uint16(now.Month()), DayOfWeek: uint16(now.Weekday()), Day: uint16(now.Day()),
		Hour: uint16(clock.Hour()), Minute: uint16(clock.Minute()),
	}
}

func (a *uiApp) isChecked(control uintptr) bool {
	if control == 0 {
		return false
	}
	result, _, _ := procSendMessageW.Call(control, bmGetCheck, 0, 0)
	return result == bstChecked
}

func (a *uiApp) getSummarySchedule() string {
	if a.summaryTime == 0 {
		return ""
	}
	var value systemTime
	result, _, _ := procSendMessageW.Call(a.summaryTime, dtmGetSystemTime, 0, uintptr(unsafe.Pointer(&value)))
	if result != gdtValid {
		return ""
	}
	return fmt.Sprintf("%02d:%02d", value.Hour, value.Minute)
}

func (a *uiApp) setSummarySchedule(schedule string) {
	if a.summaryTime == 0 {
		return
	}
	parsed, err := time.Parse("15:04", strings.TrimSpace(schedule))
	if err != nil {
		procSendMessageW.Call(a.summaryTime, dtmSetSystemTime, gdtNone, 0)
		return
	}
	value := clockSystemTime(parsed)
	procSendMessageW.Call(a.summaryTime, dtmSetSystemTime, gdtValid, uintptr(unsafe.Pointer(&value)))
}

func fill(hdc uintptr, area rect, color uint32) {
	brush := uintptr(0)
	if app != nil {
		brush = app.themeBrush(color)
	}
	if brush == 0 {
		brush, _, _ = procCreateSolidBrush.Call(uintptr(color))
		defer procDeleteObject.Call(brush)
	}
	procFillRect.Call(hdc, uintptr(unsafe.Pointer(&area)), brush)
}

func fillRounded(hdc uintptr, area rect, color uint32, radius int32) {
	brush, pen := uintptr(0), uintptr(0)
	if app != nil {
		brush, pen = app.themeBrush(color), app.themePen(color)
	}
	transientBrush, transientPen := false, false
	if brush == 0 {
		brush, _, _ = procCreateSolidBrush.Call(uintptr(color))
		transientBrush = true
	}
	if pen == 0 {
		pen, _, _ = procCreatePen.Call(psSolid, 1, uintptr(color))
		transientPen = true
	}
	oldBrush, _, _ := procSelectObject.Call(hdc, brush)
	oldPen, _, _ := procSelectObject.Call(hdc, pen)
	procRoundRect.Call(hdc, uintptr(area.Left), uintptr(area.Top), uintptr(area.Right), uintptr(area.Bottom), uintptr(radius*2), uintptr(radius*2))
	procSelectObject.Call(hdc, oldBrush)
	procSelectObject.Call(hdc, oldPen)
	if transientBrush {
		procDeleteObject.Call(brush)
	}
	if transientPen {
		procDeleteObject.Call(pen)
	}
}

func stroke(hdc uintptr, area rect, color uint32) {
	brush := uintptr(0)
	if app != nil {
		brush = app.themeBrush(color)
	}
	transient := false
	if brush == 0 {
		brush, _, _ = procCreateSolidBrush.Call(uintptr(color))
		transient = true
	}
	procFrameRect.Call(hdc, uintptr(unsafe.Pointer(&area)), brush)
	if transient {
		procDeleteObject.Call(brush)
	}
}

func drawText(hdc uintptr, area rect, text string, color uint32, font uintptr, flags uint32) {
	procSetBkMode.Call(hdc, transparent)
	procSetTextColor.Call(hdc, uintptr(color))
	old, _, _ := procSelectObject.Call(hdc, font)
	procDrawTextW.Call(hdc, uintptr(unsafe.Pointer(utf16(text))), ^uintptr(0), uintptr(unsafe.Pointer(&area)), uintptr(flags))
	procSelectObject.Call(hdc, old)
}

func button(hdc uintptr, area rect, label string, primary bool) {
	if app != nil {
		label = app.animatedButtonLabel(label)
	}
	background, foreground := rgb(255, 255, 255), rgb(10, 10, 10)
	hovered, pressed := false, false
	if app != nil && app.mouseInside && hit(app.hoverX, app.hoverY, area) {
		hovered = true
		pressed = app.mouseDown
	}
	if primary {
		background, foreground = rgb(10, 10, 10), rgb(255, 255, 255)
		if hovered {
			background = rgb(38, 38, 38)
		}
		if pressed {
			background = rgb(70, 70, 70)
		}
	} else if hovered {
		background = rgb(246, 246, 246)
		if pressed {
			background = rgb(236, 236, 236)
		}
	}
	fillRounded(hdc, area, background, 6)
	if !primary {
		stroke(hdc, area, rgb(218, 218, 218))
	}
	drawText(hdc, area, label, foreground, app.fontMedium, dtCenter|dtVCenter|dtSingleLine|dtEndEllipsis)
}

func statusPill(hdc uintptr, area rect, label, status string, online, busy bool) {
	background, foreground, border := rgb(245, 245, 245), rgb(82, 82, 82), rgb(229, 229, 229)
	switch {
	case busy:
		background, foreground, border = rgb(255, 247, 237), rgb(154, 52, 18), rgb(254, 215, 170)
	case online:
		background, foreground, border = rgb(240, 253, 244), rgb(22, 101, 52), rgb(187, 247, 208)
	case strings.Contains(status, "失败") || strings.Contains(status, "异常"):
		background, foreground, border = rgb(254, 242, 242), rgb(185, 28, 28), rgb(254, 202, 202)
	}
	fillRounded(hdc, area, background, 6)
	stroke(hdc, area, border)
	drawText(hdc, area, label, foreground, app.fontMedium, dtCenter|dtVCenter|dtSingleLine|dtEndEllipsis)
}

func fieldLabel(hdc uintptr, x, y int32, label string) {
	drawText(hdc, rect{x, y, x + 380, y + 24}, label, rgb(90, 90, 90), app.fontBody, dtLeft|dtVCenter|dtSingleLine)
}

func sectionTitle(hdc uintptr, area rect, title string, font uintptr) {
	drawText(hdc, area, title, rgb(10, 10, 10), font, dtLeft|dtVCenter|dtSingleLine)
}

func rowHeader(hdc uintptr, area rect, values []string, widths []int32) {
	fill(hdc, area, rgb(250, 250, 250))
	stroke(hdc, area, rgb(232, 232, 232))
	drawColumns(hdc, area, values, widths, rgb(100, 100, 100), app.fontMedium)
}

func tableRow(hdc uintptr, area rect, values []string, widths []int32) {
	fill(hdc, rect{area.Left, area.Bottom - 1, area.Right, area.Bottom}, rgb(232, 232, 232))
	drawColumns(hdc, area, values, widths, rgb(45, 45, 45), app.fontBody)
}

func drawColumns(hdc uintptr, area rect, values []string, widths []int32, color uint32, font uintptr) {
	x := area.Left + 14
	for index, value := range values {
		width := area.Right - x - 14
		if index < len(widths) {
			width = widths[index]
		}
		drawText(hdc, rect{x, area.Top, min(x+width, area.Right-10), area.Bottom}, value, color, font, dtLeft|dtVCenter|dtSingleLine|dtEndEllipsis)
		x += width
	}
}

func emptyState(hdc uintptr, area rect, value string) {
	drawText(hdc, area, value, rgb(135, 135, 135), app.fontBody, dtCenter|dtVCenter|dtSingleLine)
}

func parseID(value string, fallback int64) int64 {
	parsed, err := strconv.ParseInt(strings.TrimSpace(value), 10, 64)
	if err != nil || parsed <= 0 {
		return fallback
	}
	return parsed
}

func nimTime(value int64) time.Time {
	if value <= 0 {
		return time.Now().UTC()
	}
	if value < 10_000_000_000 {
		return time.Unix(value, 0).UTC()
	}
	return time.UnixMilli(value).UTC()
}

func defaultUserPath(name string) string {
	base, err := os.UserConfigDir()
	if err != nil || base == "" {
		base = "."
	}
	return filepath.Join(base, "DH", name)
}

func errorText(err error) string {
	if err == nil {
		return "NIM 尚未就绪"
	}
	return localizeUserText(err.Error())
}

func boolText(value bool) string { return onOff(value, "开启", "关闭") }

func messageKindText(kind groupmgr.MessageKind) string {
	switch kind {
	case groupmgr.MessageText:
		return "文本"
	case groupmgr.MessageImage:
		return "图片"
	case groupmgr.MessageCard:
		return "名片"
	default:
		return "其他"
	}
}

func cardJoinSourceText(source groupmgr.JoinSource) string {
	switch source {
	case groupmgr.JoinOnline:
		return "在线入群"
	case groupmgr.JoinOffline:
		return "离线新增"
	case groupmgr.JoinMessage:
		return "发言发现"
	default:
		return "初始成员"
	}
}

func cardStatusText(status groupmgr.CardStatus) string {
	switch status {
	case groupmgr.CardPlanned:
		return "待确认"
	case groupmgr.CardQueued:
		return "待处理"
	case groupmgr.CardRunning:
		return "处理中"
	case groupmgr.CardVerified:
		return "已规范"
	case groupmgr.CardFailed:
		return "失败待重试"
	case groupmgr.CardExcluded:
		return "已排除"
	case groupmgr.CardConflict:
		return "名称冲突"
	default:
		return "未规范"
	}
}

func memberIdentityText(member groupmgr.Member) string {
	if member.UserID > 0 {
		return strconv.FormatInt(member.UserID, 10)
	}
	if member.NIMID != "" {
		return "NIM " + member.NIMID
	}
	return "-"
}

func actionText(action groupmgr.ActionType) string {
	switch action {
	case groupmgr.ActionReply:
		return "回复"
	case groupmgr.ActionMute:
		return "禁言"
	case groupmgr.ActionUnmute:
		return "解除禁言"
	case groupmgr.ActionRemove:
		return "移出群聊"
	case groupmgr.ActionRecall:
		return "撤回消息"
	case groupmgr.ActionBlacklist:
		return "加入黑名单"
	case groupmgr.ActionNotify:
		return "发送通知"
	case groupmgr.ActionRename:
		return "恢复群名片"
	case groupmgr.ActionGroupMute:
		return "全员禁言"
	default:
		return "其他"
	}
}

func matcherText(matcher groupmgr.MatcherType) string {
	switch matcher {
	case groupmgr.MatcherExact:
		return "精确"
	case groupmgr.MatcherContains:
		return "包含"
	case groupmgr.MatcherPrefix:
		return "前缀"
	case groupmgr.MatcherRegex:
		return "正则"
	case groupmgr.MatcherLength:
		return "字符数"
	case groupmgr.MatcherLines:
		return "行数"
	case groupmgr.MatcherImageCount:
		return "图片次数"
	case groupmgr.MatcherSemantic:
		return "AI 语义"
	case groupmgr.MatcherRenameCount:
		return "改名次数"
	case groupmgr.MatcherBlacklist:
		return "黑名单"
	default:
		return "其他"
	}
}

func ruleModeText(mode groupmgr.RuleMode) string {
	switch mode {
	case groupmgr.RuleAuto:
		return "自动执行"
	case groupmgr.RuleObserve:
		return "仅观察"
	case groupmgr.RuleDryRun:
		return "测试模式"
	default:
		return "未知"
	}
}

func ruleEnabledText(enabled bool) string {
	if enabled {
		return "已启用"
	}
	return "已停用"
}

func knowledgeKindText(kind string) string {
	switch strings.ToLower(strings.TrimSpace(kind)) {
	case "faq":
		return "问答"
	case "document":
		return "文档"
	case "rule", "rules":
		return "群规"
	default:
		return "其他"
	}
}

func taskStatusText(status groupmgr.TaskStatus) string {
	switch status {
	case groupmgr.TaskPending:
		return "待办"
	case groupmgr.TaskCompleted:
		return "完成"
	case groupmgr.TaskCancelled:
		return "取消"
	default:
		return "未知"
	}
}

func auditLevelText(level string) string {
	switch strings.ToLower(strings.TrimSpace(level)) {
	case "info":
		return "信息"
	case "warn", "warning":
		return "警告"
	case "error":
		return "错误"
	default:
		return "记录"
	}
}

func auditEventText(event string) string {
	if label := auditEventLabels[event]; label != "" {
		return label
	}
	if strings.HasPrefix(event, "manual_") {
		action := actionText(groupmgr.ActionType(strings.TrimPrefix(event, "manual_")))
		if action != "其他" {
			return "手动" + action
		}
		return "手动操作"
	}
	return firstNonEmpty(event, "系统记录")
}

func localizeUserText(value string) string {
	trimmed := strings.TrimSpace(value)
	if trimmed == "" {
		return ""
	}
	if translated := localizedUserText[trimmed]; translated != "" {
		return translated
	}
	return userTextReplacer.Replace(trimmed)
}
func onOff(value bool, yes, no string) string {
	if value {
		return yes
	}
	return no
}
func idText(value int64) string {
	if value == 0 {
		return "-"
	}
	return strconv.FormatInt(value, 10)
}
func clock(value time.Time) string {
	if value.IsZero() {
		return "-"
	}
	return value.Local().Format("15:04:05")
}
func firstNonEmpty(values ...string) string {
	for _, value := range values {
		if strings.TrimSpace(value) != "" {
			return value
		}
	}
	return ""
}

func memberDisplayName(member groupmgr.Member) string {
	if label, inactive := appcore.InactiveMemberLabel(member); inactive {
		return label
	}
	for _, value := range []string{member.CardName, member.Nickname} {
		value = strings.TrimSpace(value)
		if value != "" && value != "1" && value != "." {
			return value
		}
	}
	return "名称未设置"
}

func hit(x, y int32, area rect) bool {
	return x >= area.Left && x <= area.Right && y >= area.Top && y <= area.Bottom
}
func rgb(r, g, b uint32) uint32  { return r | g<<8 | b<<16 }
func utf16(value string) *uint16 { pointer, _ := syscall.UTF16PtrFromString(value); return pointer }

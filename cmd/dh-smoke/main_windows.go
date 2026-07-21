//go:build windows

package main

import (
	"fmt"
	"os"
	"strconv"
	"syscall"
	"time"
	"unsafe"
)

const (
	wmLButtonDown  = 0x0201
	wmLButtonUp    = 0x0202
	wmLButtonDbl   = 0x0203
	wmClose        = 0x0010
	wmGetMinMax    = 0x0024
	wmAppTray      = 0x8002
	wmAppTrayProbe = 0x8003
)

const (
	lvmFirst            = 0x1000
	lvmGetItemCount     = lvmFirst + 4
	lvmGetHeader        = lvmFirst + 31
	lvmGetItemW         = lvmFirst + 75
	lvmGetItemTextW     = lvmFirst + 115
	lvmSetItemState     = lvmFirst + 43
	lvmGetNextItem      = lvmFirst + 12
	lvmGetExtendedStyle = lvmFirst + 55
	lvisFocused         = 0x0001
	lvisSelected        = 0x0002
	lvniSelected        = 0x0002
	lvsExCheckboxes     = 0x00000004
	lvifText            = 0x0001
	bmGetCheck          = 0x00F0
	bstChecked          = 1
	hdmGetItemCount     = 0x1200
)

type rect struct{ Left, Top, Right, Bottom int32 }
type point struct{ X, Y int32 }
type minMaxInfo struct {
	Reserved, MaxSize, MaxPosition, MinTrackSize, MaxTrackSize point
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

var (
	user32              = syscall.NewLazyDLL("user32.dll")
	procFindWindowW     = user32.NewProc("FindWindowW")
	procGetClientRect   = user32.NewProc("GetClientRect")
	procGetWindowRect   = user32.NewProc("GetWindowRect")
	procGetDlgItem      = user32.NewProc("GetDlgItem")
	procIsWindowVisible = user32.NewProc("IsWindowVisible")
	procSetWindowTextW  = user32.NewProc("SetWindowTextW")
	procSendMessageW    = user32.NewProc("SendMessageW")
	procShowWindow      = user32.NewProc("ShowWindow")
	procSetForeground   = user32.NewProc("SetForegroundWindow")
	procSetWindowPos    = user32.NewProc("SetWindowPos")
	procRegisterMessage = user32.NewProc("RegisterWindowMessageW")
)

func main() {
	hwnd, _, _ := procFindWindowW.Call(0, uintptr(unsafe.Pointer(utf16("DH BOT"))))
	if hwnd == 0 {
		fmt.Fprintln(os.Stderr, "DH BOT window not found")
		os.Exit(2)
	}
	procShowWindow.Call(hwnd, 9)
	procSetWindowPos.Call(hwnd, ^uintptr(0), 100, 80, 1260, 820, 0x0040)
	procSetForeground.Call(hwnd)
	var client rect
	procGetClientRect.Call(hwnd, uintptr(unsafe.Pointer(&client)))
	mode := "pages"
	if len(os.Args) > 1 {
		mode = os.Args[1]
	}
	switch mode {
	case "page":
		requireArgs(3, "page requires INDEX")
		index := mustIndex(os.Args[2], 0, 8, "page index must be 0..8")
		openPage(hwnd, index)
	case "ai-test":
		openPage(hwnd, 7)
		click(hwnd, 621, 539)
		time.Sleep(time.Second)
	case "connect":
		click(hwnd, client.Right-90, 39)
		time.Sleep(3 * time.Second)
	case "configure":
		requireArgs(4, "configure requires DEVTOOLS_URL ENDPOINT")
		openPage(hwnd, 7)
		setEdit(hwnd, 2501, os.Args[2])
		setEdit(hwnd, 2502, os.Args[3])
		click(hwnd, 337, 449)
		time.Sleep(500 * time.Millisecond)
	case "configure-ai":
		requireArgs(6, "configure-ai requires KIND BASE_OR_WEBHOOK MODEL API_KEY")
		openPage(hwnd, 7)
		setEdit(hwnd, 2504, os.Args[2])
		if os.Args[2] == "webhook" {
			setEdit(hwnd, 2508, os.Args[3])
		} else {
			setEdit(hwnd, 2505, os.Args[3])
		}
		setEdit(hwnd, 2506, os.Args[4])
		setEdit(hwnd, 2507, os.Args[5])
		click(hwnd, 337, 449)
		time.Sleep(500 * time.Millisecond)
	case "select-group":
		requireArgs(3, "select-group requires ROW_INDEX")
		index := mustIndex(os.Args[2], 0, 3, "group row index must be 0..3")
		openPage(hwnd, 1)
		selectList(hwnd, 3002, index)
		time.Sleep(time.Second)
	case "inspect-group":
		openPage(hwnd, 1)
		time.Sleep(time.Second)
		fmt.Printf("selected_group=%d\n", selectedListID(hwnd, 3002))
	case "inspect-members":
		openPage(hwnd, 1)
		click(hwnd, 454, 244)
		time.Sleep(time.Second)
		members := requireVisibleControl(hwnd, 3003, "member list")
		count, _, _ := procSendMessageW.Call(members, lvmGetItemCount, 0, 0)
		fmt.Printf("member_rows=%d\n", count)
	case "inspect-rules":
		openPage(hwnd, 3)
		time.Sleep(time.Second)
		rules := requireVisibleControl(hwnd, 3005, "rules list")
		count, _, _ := procSendMessageW.Call(rules, lvmGetItemCount, 0, 0)
		fmt.Printf("rule_rows=%d\n", count)
	case "sync":
		openPage(hwnd, 1)
		click(hwnd, 292, 196)
		time.Sleep(4 * time.Second)
	case "toggle-group":
		requireArgs(3, "toggle-group requires enabled|ai|moderation|manual")
		openPage(hwnd, 1)
		x := map[string]int32{"enabled": 420, "ai": 525, "moderation": 626, "manual": 736}[os.Args[2]]
		if x == 0 {
			fail("unknown group toggle")
		}
		click(hwnd, x, 196)
		time.Sleep(400 * time.Millisecond)
	case "add-rule":
		requireArgs(5, "add-rule requires GROUP_ID PATTERN ACTION")
		openPage(hwnd, 3)
		setEdit(hwnd, 2201, os.Args[2])
		setEdit(hwnd, 2202, os.Args[3])
		setEdit(hwnd, 2203, os.Args[4])
		click(hwnd, client.Right-90, 147)
		time.Sleep(400 * time.Millisecond)
	case "add-knowledge":
		requireArgs(5, "add-knowledge requires GROUP_ID TITLE CONTENT")
		openPage(hwnd, 4)
		setEdit(hwnd, 2301, os.Args[2])
		setEdit(hwnd, 2302, os.Args[3])
		setEdit(hwnd, 2304, os.Args[4])
		click(hwnd, client.Right-226, 345)
		time.Sleep(400 * time.Millisecond)
	case "add-task":
		requireArgs(5, "add-task requires GROUP_ID TITLE ASSIGNEE_ID")
		openPage(hwnd, 5)
		setEdit(hwnd, 2401, os.Args[2])
		setEdit(hwnd, 2402, os.Args[3])
		setEdit(hwnd, 2403, os.Args[4])
		click(hwnd, client.Right-226, 209)
		time.Sleep(400 * time.Millisecond)
	case "pages":
		for index := 0; index < 8; index++ {
			openPage(hwnd, index)
			time.Sleep(200 * time.Millisecond)
		}
	case "validate-v23":
		openPage(hwnd, 1)
		requireVisibleControl(hwnd, 2002, "summary time picker")
		click(hwnd, 454, 244)
		time.Sleep(300 * time.Millisecond)
		members := requireVisibleControl(hwnd, 3003, "member list")
		extended, _, _ := procSendMessageW.Call(members, lvmGetExtendedStyle, 0, 0)
		if extended&lvsExCheckboxes == 0 {
			fail("member list checkboxes are disabled")
		}
		openPage(hwnd, 3)
		requireVisibleControl(hwnd, 2701, "recall checkbox")
		requireVisibleControl(hwnd, 2702, "mute checkbox")
		openPage(hwnd, 0)
		requireVisibleControl(hwnd, 3009, "summary list")
	case "validate-v25", "validate-v24":
		if tray, _, _ := procSendMessageW.Call(hwnd, wmAppTrayProbe, 0, 0); tray == 0 {
			fail("tray icon registration failed")
		}
		taskbarCreated, _, _ := procRegisterMessage.Call(uintptr(unsafe.Pointer(utf16("TaskbarCreated"))))
		procSendMessageW.Call(hwnd, taskbarCreated, 0, 0)
		if tray, _, _ := procSendMessageW.Call(hwnd, wmAppTrayProbe, 0, 0); tray == 0 {
			fail("tray icon did not recover after TaskbarCreated")
		}
		var limits minMaxInfo
		procSendMessageW.Call(hwnd, wmGetMinMax, 0, uintptr(unsafe.Pointer(&limits)))
		if limits.MinTrackSize.X != 1180 || limits.MinTrackSize.Y != 760 {
			fail(fmt.Sprintf("minimum window=%dx%d, want 1180x760", limits.MinTrackSize.X, limits.MinTrackSize.Y))
		}
		procSetWindowPos.Call(hwnd, 0, 100, 80, 1180, 760, 0x0040)
		time.Sleep(300 * time.Millisecond)
		openPage(hwnd, 1)
		selectList(hwnd, 3002, 0)
		time.Sleep(300 * time.Millisecond)
		click(hwnd, 454, 286)
		time.Sleep(300 * time.Millisecond)
		requireControlInside(hwnd, 3003, "member list")
		requireControlInside(hwnd, 2003, "mute duration")
		requireControlInside(hwnd, 2005, "card suggestion")
		openPage(hwnd, 3)
		time.Sleep(300 * time.Millisecond)
		recallCheck := requireVisibleControl(hwnd, 2701, "recall checkbox")
		muteCheck := requireVisibleControl(hwnd, 2702, "mute checkbox")
		if checked, _, _ := procSendMessageW.Call(recallCheck, bmGetCheck, 0, 0); checked != bstChecked {
			fail("recall checkbox is not checked by default")
		}
		if checked, _, _ := procSendMessageW.Call(muteCheck, bmGetCheck, 0, 0); checked == bstChecked {
			fail("mute checkbox is checked by default")
		}
		rules := requireVisibleControl(hwnd, 3005, "rules list")
		count, _, _ := procSendMessageW.Call(rules, lvmGetItemCount, 0, 0)
		if count == 0 {
			fail("rules list is empty")
		}
		header, _, _ := procSendMessageW.Call(rules, lvmGetHeader, 0, 0)
		columns, _, _ := procSendMessageW.Call(header, hdmGetItemCount, 0, 0)
		if columns != 7 {
			fail(fmt.Sprintf("rules columns=%d, want 7", columns))
		}
		procSendMessageW.Call(hwnd, wmClose, 0, 0)
		time.Sleep(200 * time.Millisecond)
		if visible, _, _ := procIsWindowVisible.Call(hwnd); visible != 0 {
			fail("window stayed visible after close-to-tray")
		}
		procSendMessageW.Call(hwnd, wmAppTray, 0, wmLButtonDbl)
		time.Sleep(200 * time.Millisecond)
		if visible, _, _ := procIsWindowVisible.Call(hwnd); visible == 0 {
			fail("window did not restore from tray callback")
		}
	case "validate-shell":
		if tray, _, _ := procSendMessageW.Call(hwnd, wmAppTrayProbe, 0, 0); tray == 0 {
			fail("tray icon registration failed")
		}
		taskbarCreated, _, _ := procRegisterMessage.Call(uintptr(unsafe.Pointer(utf16("TaskbarCreated"))))
		procSendMessageW.Call(hwnd, taskbarCreated, 0, 0)
		if tray, _, _ := procSendMessageW.Call(hwnd, wmAppTrayProbe, 0, 0); tray == 0 {
			fail("tray icon did not recover after TaskbarCreated")
		}
		var limits minMaxInfo
		procSendMessageW.Call(hwnd, wmGetMinMax, 0, uintptr(unsafe.Pointer(&limits)))
		if limits.MinTrackSize.X != 1180 || limits.MinTrackSize.Y != 760 {
			fail(fmt.Sprintf("minimum window=%dx%d, want 1180x760", limits.MinTrackSize.X, limits.MinTrackSize.Y))
		}
		openPage(hwnd, 5)
		requireVisibleControl(hwnd, 2511, "schedule open time")
		requireVisibleControl(hwnd, 2512, "schedule close time")
		requireVisibleControl(hwnd, 2705, "schedule enabled checkbox")
		openPage(hwnd, 3)
		requireVisibleControl(hwnd, 2701, "recall checkbox")
		requireVisibleControl(hwnd, 2702, "mute checkbox")
	default:
		fail("usage: DH-smoke.exe {page INDEX|connect|configure DEVTOOLS_URL ENDPOINT|configure-ai KIND BASE_OR_WEBHOOK MODEL API_KEY|select-group ROW_INDEX|sync|toggle-group enabled|ai|moderation|manual|add-rule GROUP_ID PATTERN ACTION|add-knowledge GROUP_ID TITLE CONTENT|add-task GROUP_ID TITLE ASSIGNEE_ID|pages}")
	}
	fmt.Printf("ok mode=%s hwnd=0x%x client=%dx%d\n", mode, hwnd, client.Right, client.Bottom)
}

func openPage(hwnd uintptr, index int) {
	click(hwnd, 100, int32(96+index*44))
}

func click(hwnd uintptr, x, y int32) {
	lParam := uintptr(uint32(uint16(x)) | uint32(uint16(y))<<16)
	procSendMessageW.Call(hwnd, wmLButtonUp, 0, lParam)
}

func setEdit(hwnd uintptr, id int, value string) {
	control, _, _ := procGetDlgItem.Call(hwnd, uintptr(id))
	if control == 0 {
		fail(fmt.Sprintf("control %d not found", id))
	}
	procSetWindowTextW.Call(control, uintptr(unsafe.Pointer(utf16(value))))
}

func requireVisibleControl(hwnd uintptr, id int, name string) uintptr {
	control, _, _ := procGetDlgItem.Call(hwnd, uintptr(id))
	if control == 0 {
		fail(name + " not found")
	}
	visible, _, _ := procIsWindowVisible.Call(control)
	if visible == 0 {
		fail(name + " is hidden")
	}
	return control
}

func requireControlInside(hwnd uintptr, id int, name string) {
	control := requireVisibleControl(hwnd, id, name)
	var windowRect, controlRect rect
	procGetWindowRect.Call(hwnd, uintptr(unsafe.Pointer(&windowRect)))
	procGetWindowRect.Call(control, uintptr(unsafe.Pointer(&controlRect)))
	if controlRect.Left < windowRect.Left || controlRect.Top < windowRect.Top || controlRect.Right > windowRect.Right || controlRect.Bottom > windowRect.Bottom {
		fail(fmt.Sprintf("%s outside window: control=%+v window=%+v", name, controlRect, windowRect))
	}
}

func selectList(hwnd uintptr, id, index int) {
	control, _, _ := procGetDlgItem.Call(hwnd, uintptr(id))
	if control == 0 {
		fail(fmt.Sprintf("list %d not found", id))
	}
	deadline := time.Now().Add(8 * time.Second)
	for time.Now().Before(deadline) {
		count, _, _ := procSendMessageW.Call(control, lvmGetItemCount, 0, 0)
		if int(count) > index {
			break
		}
		time.Sleep(100 * time.Millisecond)
	}
	clear := listViewItem{StateMask: lvisSelected | lvisFocused}
	procSendMessageW.Call(control, lvmSetItemState, ^uintptr(0), uintptr(unsafe.Pointer(&clear)))
	selection := listViewItem{State: lvisSelected | lvisFocused, StateMask: lvisSelected | lvisFocused}
	var selected uintptr
	deadline = time.Now().Add(1200 * time.Millisecond)
	for time.Now().Before(deadline) {
		procSendMessageW.Call(control, lvmSetItemState, uintptr(index), uintptr(unsafe.Pointer(&selection)))
		selected, _, _ = procSendMessageW.Call(control, lvmGetNextItem, ^uintptr(0), lvniSelected)
		time.Sleep(50 * time.Millisecond)
	}
	if int(int32(selected)) != index {
		y := int32(28 + index*20)
		lParam := uintptr(uint32(uint16(120)) | uint32(uint16(y))<<16)
		procSendMessageW.Call(control, wmLButtonDown, 1, lParam)
		procSendMessageW.Call(control, wmLButtonUp, 0, lParam)
		selected, _, _ = procSendMessageW.Call(control, lvmGetNextItem, ^uintptr(0), lvniSelected)
	}
	if int(int32(selected)) != index {
		fail(fmt.Sprintf("list %d selected row %d, want %d", id, int(int32(selected)), index))
	}
}

func selectedListID(hwnd uintptr, id int) int64 {
	control, _, _ := procGetDlgItem.Call(hwnd, uintptr(id))
	if control == 0 {
		fail(fmt.Sprintf("list %d not found", id))
	}
	selected, _, _ := procSendMessageW.Call(control, lvmGetNextItem, ^uintptr(0), lvniSelected)
	index := int(int32(selected))
	if index < 0 {
		return 0
	}
	item := listViewItem{Mask: 0x0004, Item: int32(index)}
	if ok, _, _ := procSendMessageW.Call(control, lvmFirst+75, 0, uintptr(unsafe.Pointer(&item))); ok == 0 {
		return 0
	}
	return int64(item.Param)
}

func listText(control uintptr, row, column int) string {
	buffer := make([]uint16, 256)
	item := listViewItem{Mask: lvifText, Item: int32(row), SubItem: int32(column), Text: &buffer[0], TextMax: int32(len(buffer))}
	procSendMessageW.Call(control, lvmGetItemTextW, uintptr(row), uintptr(unsafe.Pointer(&item)))
	return syscall.UTF16ToString(buffer)
}

func mustIndex(value string, minValue, maxValue int, message string) int {
	index, err := strconv.Atoi(value)
	if err != nil || index < minValue || index > maxValue {
		fail(message)
	}
	return index
}

func requireArgs(count int, message string) {
	if len(os.Args) < count {
		fail(message)
	}
}

func fail(message string) {
	fmt.Fprintln(os.Stderr, message)
	os.Exit(2)
}

func utf16(value string) *uint16 {
	pointer, _ := syscall.UTF16PtrFromString(value)
	return pointer
}

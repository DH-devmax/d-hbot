package wslcontrol

import (
	"context"
	"time"
)

type InstallCandidate struct {
	Path   string
	Source string
}

type ProcessRef struct {
	PID       int
	ImagePath string
	StartedAt time.Time
}

type StartOptions struct {
	ImagePath   string
	DevToolsURL string
	ProfileID   string
}

type ActivationResult struct {
	PID       int
	Window    uintptr
	Title     string
	Activated bool
}

type ProfileStatus struct {
	State       string
	ScriptPath  string
	BackupPath  string
	ProfileRoot string
	ProfileID   string
	OriginalSHA string
	PatchedSHA  string
	Migrated    bool
	Detail      string
}

type DevToolsStatus string

const (
	DevToolsReady        DevToolsStatus = "ready"
	DevToolsUnavailable  DevToolsStatus = "unavailable"
	DevToolsOtherService DevToolsStatus = "other-service"
)

type Controller interface {
	Locate(context.Context) ([]InstallCandidate, error)
	Inspect(context.Context, string) (DevToolsStatus, error)
	ListProcesses(context.Context, string) ([]ProcessRef, error)
	Start(context.Context, StartOptions) (ProcessRef, error)
	Stop(context.Context, ProcessRef) error
	Activate(context.Context, string) (ActivationResult, error)
	PreparePersistentProfile(context.Context, string, string) (ProfileStatus, error)
	RestorePersistentProfile(context.Context, string, string) error
}

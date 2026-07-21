//go:build !windows

package wslcontrol

import (
	"context"
	"errors"
)

type Unsupported struct{}

func (Unsupported) Locate(context.Context) ([]InstallCandidate, error) {
	return nil, errors.New("旺商聊自动启动仅支持 Windows")
}
func (Unsupported) Inspect(context.Context, string) (DevToolsStatus, error) {
	return DevToolsUnavailable, errors.New("旺商聊自动启动仅支持 Windows")
}
func (Unsupported) ListProcesses(context.Context, string) ([]ProcessRef, error) {
	return nil, errors.New("旺商聊自动启动仅支持 Windows")
}
func (Unsupported) Start(context.Context, StartOptions) (ProcessRef, error) {
	return ProcessRef{}, errors.New("旺商聊自动启动仅支持 Windows")
}
func (Unsupported) Stop(context.Context, ProcessRef) error {
	return errors.New("旺商聊自动启动仅支持 Windows")
}
func (Unsupported) Activate(context.Context, string) (ActivationResult, error) {
	return ActivationResult{}, errors.New("旺商聊窗口唤起仅支持 Windows")
}
func (Unsupported) PreparePersistentProfile(context.Context, string, string) (ProfileStatus, error) {
	return ProfileStatus{}, errors.New("旺商聊登录分区仅支持 Windows")
}
func (Unsupported) RestorePersistentProfile(context.Context, string, string) error {
	return errors.New("旺商聊登录分区仅支持 Windows")
}

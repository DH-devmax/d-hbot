package groupmgr

import (
	"context"
	"errors"
	"fmt"
	"time"
)

var (
	ErrPermissionDenied = errors.New("group gateway: permission denied")
	ErrBusiness         = errors.New("group gateway: business error")
	ErrNotFound         = errors.New("group gateway: not found")
)

type GroupGateway interface {
	ListGroups(context.Context) ([]Group, error)
	ListMembers(context.Context, int64) (MemberRoster, error)
	SendText(context.Context, int64, string) (string, error)
	Recall(context.Context, int64, int64, string) error
	Mute(context.Context, int64, int64, time.Duration) error
	Unmute(context.Context, int64, int64) error
	Rename(context.Context, int64, MemberRef, string) error
	RemoveMember(context.Context, int64, int64) error
	SetGroupMute(context.Context, int64, bool) error
}

type MemberRef struct {
	UserID int64  `json:"userId,omitempty"`
	NIMID  string `json:"nimId,omitempty"`
}

func (ref MemberRef) Valid() bool {
	return ref.UserID > 0 || ref.NIMID != ""
}

type MemberRoster struct {
	Members       []Member `json:"members"`
	ReportedCount int      `json:"reportedCount"`
	ResolvedCount int      `json:"resolvedCount"`
	Complete      bool     `json:"complete"`
	Sources       []string `json:"sources,omitempty"`
}

type GroupResolver interface {
	ResolveGroupID(context.Context, string) (int64, error)
}

type GatewayError struct {
	Op      string
	Code    string
	Message string
	Kind    error
}

func (e *GatewayError) Error() string {
	operation := map[string]string{
		"list groups": "读取群列表", "resolve group": "解析群身份", "list members": "读取群成员",
		"send text": "发送文本", "recall": "撤回消息", "mute": "禁言", "unmute": "解除禁言",
		"rename": "修改群名片", "rename nim": "通过 NIM 修改群名片", "remove member": "移出群聊", "set group mute": "全员禁言",
	}[e.Op]
	if operation == "" {
		operation = e.Op
	}
	if e.Message == "" {
		return fmt.Sprintf("群管理操作“%s”失败：%s", operation, e.Code)
	}
	return fmt.Sprintf("群管理操作“%s”失败：%s（%s）", operation, e.Message, e.Code)
}

func (e *GatewayError) Unwrap() error {
	if e.Kind == nil {
		return ErrBusiness
	}
	return e.Kind
}

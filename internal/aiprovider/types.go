package aiprovider

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strings"
)

const ContractVersion = "1"

type Provider interface {
	Decide(ctx context.Context, request Request) (Decision, error)
}

type Group struct {
	ID   int64  `json:"id"`
	Name string `json:"name,omitempty"`
}

type Member struct {
	UserID int64  `json:"userId"`
	Name   string `json:"name,omitempty"`
	Role   string `json:"role,omitempty"`
}

type Message struct {
	IDServer string `json:"idServer,omitempty"`
	Format   string `json:"format,omitempty"`
	Text     string `json:"text,omitempty"`
	Time     int64  `json:"time,omitempty"`
}

type ContextMessage struct {
	UserID int64  `json:"userId"`
	Name   string `json:"name,omitempty"`
	Text   string `json:"text"`
	Time   int64  `json:"time,omitempty"`
}

type KnowledgeChunk struct {
	Source string `json:"source"`
	Title  string `json:"title,omitempty"`
	Base   string `json:"base,omitempty"`
	Text   string `json:"text"`
}

type Request struct {
	Version       string           `json:"version"`
	EventID       string           `json:"eventId"`
	Persona       string           `json:"persona,omitempty"`
	Group         Group            `json:"group"`
	Member        Member           `json:"member"`
	Message       Message          `json:"message"`
	RecentContext []ContextMessage `json:"recentContext,omitempty"`
	Knowledge     []KnowledgeChunk `json:"knowledge,omitempty"`
}

type Action struct {
	Type            string  `json:"type"`
	Category        string  `json:"category,omitempty"`
	GroupID         int64   `json:"groupId"`
	TargetUserID    int64   `json:"targetUserId,omitempty"`
	MessageID       string  `json:"messageId,omitempty"`
	DurationMinutes int     `json:"durationMinutes,omitempty"`
	Nickname        string  `json:"nickname,omitempty"`
	Reason          string  `json:"reason,omitempty"`
	Confidence      float64 `json:"confidence,omitempty"`
}

type Task struct {
	Title       string `json:"title"`
	AssigneeID  int64  `json:"assigneeId,omitempty"`
	DueAt       int64  `json:"dueAt,omitempty"`
	Description string `json:"description,omitempty"`
}

type Decision struct {
	Reply      string   `json:"reply,omitempty"`
	Actions    []Action `json:"actions,omitempty"`
	Tasks      []Task   `json:"tasks,omitempty"`
	Confidence float64  `json:"confidence,omitempty"`
	Reason     string   `json:"reason,omitempty"`
}

var allowedActions = map[string]bool{
	"recall": true, "mute": true, "unmute": true, "remove": true,
	"rename": true, "blacklist": true, "ignore": true,
}

func ValidateRequest(request Request) error {
	if request.Version != ContractVersion {
		return fmt.Errorf("不支持的协议版本 %q", request.Version)
	}
	if strings.TrimSpace(request.EventID) == "" {
		return errors.New("缺少事件 ID")
	}
	if request.Group.ID <= 0 || request.Member.UserID <= 0 {
		return errors.New("缺少群 ID 或成员 ID")
	}
	if strings.TrimSpace(request.Message.Text) == "" && request.Message.Format == "" {
		return errors.New("消息内容为空")
	}
	return nil
}

func ValidateDecision(request Request, decision Decision) error {
	if decision.Confidence < 0 || decision.Confidence > 1 {
		return errors.New("AI 决策置信度必须在 0 到 1 之间")
	}
	for index, action := range decision.Actions {
		if !allowedActions[action.Type] {
			return fmt.Errorf("第 %d 个 AI 动作类型 %q 不受支持", index+1, action.Type)
		}
		if action.GroupID != request.Group.ID {
			return fmt.Errorf("第 %d 个 AI 动作的群 ID 与请求不一致", index+1)
		}
		if action.Confidence < 0 || action.Confidence > 1 {
			return fmt.Errorf("第 %d 个 AI 动作的置信度必须在 0 到 1 之间", index+1)
		}
		if action.Category != strings.TrimSpace(action.Category) || len(action.Category) > 80 || strings.ContainsAny(action.Category, "\r\n\t") {
			return fmt.Errorf("第 %d 个 AI 动作的分类字段无效", index+1)
		}
		switch action.Type {
		case "mute", "unmute", "remove", "rename", "blacklist":
			if action.TargetUserID <= 0 {
				return fmt.Errorf("第 %d 个 AI 动作缺少目标成员 ID", index+1)
			}
		}
		if action.Type == "mute" && (action.DurationMinutes < 1 || action.DurationMinutes > 43200) {
			return fmt.Errorf("第 %d 个 AI 动作的禁言时长必须在 1 到 43200 分钟之间", index+1)
		}
		if action.Type == "recall" && strings.TrimSpace(action.MessageID) == "" {
			return fmt.Errorf("第 %d 个 AI 撤回动作缺少消息 ID", index+1)
		}
		if action.Type == "rename" && strings.TrimSpace(action.Nickname) == "" {
			return fmt.Errorf("第 %d 个 AI 改名动作缺少新名称", index+1)
		}
	}
	for index, task := range decision.Tasks {
		if strings.TrimSpace(task.Title) == "" {
			return fmt.Errorf("第 %d 个 AI 任务缺少标题", index+1)
		}
	}
	return nil
}

func decodeDecision(raw []byte, destination *Decision) error {
	decoder := json.NewDecoder(strings.NewReader(string(raw)))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(destination); err != nil {
		return fmt.Errorf("解析 AI 返回内容失败：%w", err)
	}
	var trailing any
	if err := decoder.Decode(&trailing); !errors.Is(err, io.EOF) {
		if err == nil {
			return errors.New("AI 返回内容包含多余 JSON 数据")
		}
		return fmt.Errorf("解析 AI 返回的多余数据失败：%w", err)
	}
	return nil
}

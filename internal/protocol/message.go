package protocol

import (
	"encoding/json"
	"strings"
	"time"
)

const (
	DeviceDesktop      = "Desktop"
	SessionP2P         = "MSG_KIND_P2P"
	SessionGroup       = "MSG_KIND_GROUP"
	AccountMember      = "ACCOUNT_MEMBER"
	FormatText         = "MSG_TEXT"
	RoleMine           = "MSG_MINE"
	RingtoneNone       = "MSG_RINGTONE_NONE"
	AppointNone        = "MSG_APPOINT_NONE"
	ObservedMsgVersion = 2
)

type MessageEndpoint struct {
	ID int64 `json:"id"`
}

type TextContent struct {
	Data string `json:"data"`
}

type DecodedEndpoint struct {
	ID   int64  `json:"id"`
	Name string `json:"name"`
}

type DecodedMessage struct {
	From       DecodedEndpoint `json:"from"`
	To         DecodedEndpoint `json:"to"`
	MsgSession int             `json:"msgSession"`
	MsgFormat  int             `json:"msgFormat"`
	Content    TextContent     `json:"content"`
	Mentions   json.RawMessage `json:"mentions"`
	Quote      json.RawMessage `json:"quote"`
}

type IncomingMessage struct {
	Seq         uint64          `json:"seq"`
	Source      string          `json:"source"`
	IDClient    string          `json:"idClient"`
	IDServer    string          `json:"idServer"`
	Scene       string          `json:"scene"`
	From        string          `json:"from"`
	To          string          `json:"to"`
	Time        int64           `json:"time"`
	Type        string          `json:"type"`
	MsgFormat   int             `json:"msgFormat"`
	Flow        string          `json:"flow"`
	Content     string          `json:"content"`
	Attach      json.RawMessage `json:"attach"`
	Mentions    json.RawMessage `json:"mentions"`
	Quote       json.RawMessage `json:"quote"`
	FromNick    string          `json:"fromNick"`
	SessionID   string          `json:"sessionId"`
	Decoded     *DecodedMessage `json:"decoded"`
	DecodeError string          `json:"decodeError"`
}

type TeamJoinNotification struct {
	GroupCloudID string
}

func ParseTeamJoinNotification(message IncomingMessage) (TeamJoinNotification, bool) {
	if message.Scene != "team" || len(message.Attach) == 0 || string(message.Attach) == "null" {
		return TeamJoinNotification{}, false
	}
	var attach struct {
		Type string `json:"type"`
		Team struct {
			TeamID string `json:"teamId"`
			ID     string `json:"id"`
		} `json:"team"`
		TeamID string `json:"teamId"`
	}
	if err := json.Unmarshal(message.Attach, &attach); err != nil {
		return TeamJoinNotification{}, false
	}
	switch strings.ToLower(strings.TrimSpace(attach.Type)) {
	case "addteammembers", "acceptteaminvite", "passteamapply":
	default:
		return TeamJoinNotification{}, false
	}
	cloudID := firstNonBlank(attach.TeamID, attach.Team.TeamID, attach.Team.ID, message.To)
	if cloudID == "" {
		return TeamJoinNotification{}, false
	}
	return TeamJoinNotification{GroupCloudID: cloudID}, true
}

func firstNonBlank(values ...string) string {
	for _, value := range values {
		if value = strings.TrimSpace(value); value != "" {
			return value
		}
	}
	return ""
}

type ListenerState struct {
	OK        bool     `json:"ok"`
	Installed []string `json:"installed"`
	Queued    int      `json:"queued"`
	Dropped   int      `json:"dropped"`
	Error     string   `json:"error"`
}

type MessageBatch struct {
	OK        bool              `json:"ok"`
	Messages  []IncomingMessage `json:"messages"`
	Remaining int               `json:"remaining"`
	Dropped   int               `json:"dropped"`
	Error     string            `json:"error"`
}

type MessageAck struct {
	OK        bool   `json:"ok"`
	Acked     int    `json:"acked"`
	Dropped   int    `json:"dropped"`
	Remaining int    `json:"remaining"`
	Error     string `json:"error"`
}

type SessionInfo struct {
	SenderID       int64  `json:"senderId"`
	DefaultGroupID int64  `json:"defaultGroupId"`
	GroupCount     int    `json:"groupCount"`
	NIMReady       bool   `json:"nimReady"`
	NIMAccount     string `json:"nimAccount"`
}

type TextMessage struct {
	From        MessageEndpoint `json:"from"`
	To          MessageEndpoint `json:"to"`
	MsgDevice   string          `json:"msgDevice"`
	CreatedAt   string          `json:"created_at"`
	MsgSession  string          `json:"msgSession"`
	MsgVersion  int             `json:"msgVersion"`
	AccountType string          `json:"accountType"`
	MsgFormat   string          `json:"msgFormat"`
	MsgRole     string          `json:"msgRole"`
	MsgRingtone string          `json:"msgRingtone"`
	Appoint     string          `json:"appoint"`
	Content     TextContent     `json:"content"`
}

func NewTextMessage(fromID, toID int64, text, session string, now time.Time) TextMessage {
	if session == "" {
		session = SessionP2P
	}
	return TextMessage{
		From: MessageEndpoint{ID: fromID}, To: MessageEndpoint{ID: toID},
		MsgDevice: DeviceDesktop, CreatedAt: now.UTC().Format(time.RFC3339),
		MsgSession: session, MsgVersion: ObservedMsgVersion,
		AccountType: AccountMember, MsgFormat: FormatText, MsgRole: RoleMine,
		MsgRingtone: RingtoneNone, Appoint: AppointNone,
		Content: TextContent{Data: text},
	}
}

package protocol

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"hash/fnv"
	"math"
	"net/http"
	"strconv"
	"strings"
	"sync"
	"time"

	"dh/internal/groupmgr"
)

const (
	GroupProtocolVersion = "0"
	GroupMuteMembers     = "MUTE_MEMBER"
	GroupMuteDisabled    = "MUTE_NO"
)

type GroupListRequest struct {
	Version string `json:"v"`
}

type GroupMembersRequest struct {
	GroupID int64  `json:"groupId"`
	Version string `json:"v"`
}

type NIMTeamMembersRequest struct {
	GroupID int64 `json:"groupId"`
}

type NIMUpdateNickRequest struct {
	GroupID int64  `json:"groupId"`
	NIMID   string `json:"nimId"`
	Nick    string `json:"nick"`
}

type MuteRequest struct {
	GroupID int64 `json:"groupId"`
	UserID  int64 `json:"userId"`
	Minutes int   `json:"min"`
}

type UnmuteRequest struct {
	GroupID int64 `json:"groupId"`
	UserID  int64 `json:"userId"`
}

type RenameRequest struct {
	GroupID int64  `json:"groupId"`
	UserID  int64  `json:"userId"`
	Nick    string `json:"nick"`
}

type RemoveMemberRequest struct {
	GroupID       int64   `json:"groupId"`
	MemberUserIDs []int64 `json:"groupMemberIds"`
}

type RecallRequest struct {
	GroupCloudID string `json:"groupCloudId"`
	UserID       int64  `json:"userId"`
	MessageID    string `json:"msgId"`
}

type SetGroupMuteRequest struct {
	GroupID  int64  `json:"groupId"`
	MuteMode string `json:"muteMode"`
}

type GroupRelation string

const (
	GroupRelationOwner  GroupRelation = "owner"
	GroupRelationMember GroupRelation = "member"
)

// GroupInfo is the wire-facing group DTO. The recovered endpoint may encode
// identifiers as either JSON numbers or decimal strings.
type GroupInfo struct {
	GroupID      int64         `json:"groupId"`
	GroupCloudID string        `json:"groupCloudId"`
	Name         string        `json:"name"`
	OwnerUserID  int64         `json:"ownerUserId"`
	MemberCount  int           `json:"memberCount"`
	Relation     GroupRelation `json:"relation"`
}

func (g *GroupInfo) UnmarshalJSON(data []byte) error {
	fields, err := rawFields(data)
	if err != nil {
		return err
	}
	if g.GroupID, err = int64Field(fields, "groupId"); err != nil {
		return fmt.Errorf("群 ID 无效：%w", err)
	}
	g.GroupCloudID, err = stringIDField(fields, "groupCloudId")
	if err != nil {
		return fmt.Errorf("群云端 ID 无效：%w", err)
	}
	g.Name = firstTextField(fields, "groupName", "name", "remarkName", "nick")
	g.OwnerUserID, err = firstInt64Field(fields, "ownerUserId", "groupOwnerId", "ownerId")
	if err != nil {
		return fmt.Errorf("群主 ID 无效：%w", err)
	}
	memberCount, countErr := firstInt64Field(fields, "memberCount", "groupMemberCount", "groupMemberNum", "memberNum", "userCount")
	if countErr == nil && memberCount > 0 {
		g.MemberCount = int(memberCount)
	}
	return nil
}

type GroupMemberInfo struct {
	GroupID      int64  `json:"groupId"`
	UserID       int64  `json:"userId"`
	NIMID        string `json:"nimId"`
	Nickname     string `json:"nickname"`
	CardName     string `json:"cardName"`
	AccountState string `json:"accountState"`
	Role         string `json:"role"`
	Account      string `json:"account"`
}

func (m *GroupMemberInfo) UnmarshalJSON(data []byte) error {
	fields, err := rawFields(data)
	if err != nil {
		return err
	}
	m.GroupID, err = int64Field(fields, "groupId")
	if err != nil {
		return fmt.Errorf("群 ID 无效：%w", err)
	}
	m.UserID, err = int64Field(fields, "userId")
	if err != nil {
		return fmt.Errorf("成员 ID 无效：%w", err)
	}
	m.NIMID, err = stringIDField(fields, "nimId")
	if err != nil {
		return fmt.Errorf("NIM ID 无效：%w", err)
	}
	m.Nickname = firstTextField(fields, "userNick", "nickname", "userName", "name")
	m.CardName = firstTextField(fields, "groupMemberNick", "nick", "groupNick", "cardName")
	m.AccountState = firstStringField(fields, "accountState", "accountStatus")
	if m.Nickname == "" {
		m.Nickname = m.CardName
	}
	if m.CardName == "" {
		m.CardName = m.Nickname
	}
	m.Role = firstStringField(fields, "groupRole", "role", "identity", "memberRole", "type")
	m.Account = firstStringField(fields, "account", "accountId", "groupAccount")
	return nil
}

type groupListData struct {
	Owner  []GroupInfo `json:"owner"`
	Member []GroupInfo `json:"member"`
}

type groupMembersData struct {
	Members []GroupMemberInfo `json:"groupMemberInfo"`
}

type nimTeamMember struct {
	NIMID    string `json:"nimId"`
	CardName string `json:"cardName"`
	Type     string `json:"type"`
	JoinTime int64  `json:"joinTime"`
}

type nimTeamMembersData struct {
	OK      bool            `json:"ok"`
	Members []nimTeamMember `json:"members"`
}

type Delivery struct {
	Delivered bool   `json:"delivered"`
	IDClient  string `json:"idClient"`
	IDServer  string `json:"idServer"`
	Status    string `json:"status"`
	Flow      string `json:"flow"`
	Scene     string `json:"scene"`
}

type HTTPGateway struct {
	client *Client

	senderMu sync.Mutex
	senderID int64
	nimID    string
}

func NewHTTPGateway(client *Client) *HTTPGateway {
	if client == nil {
		client = NewClient("", Credentials{})
	}
	return &HTTPGateway{client: client}
}

func (g *HTTPGateway) SetSenderID(senderID int64) {
	g.senderMu.Lock()
	g.senderID = senderID
	g.senderMu.Unlock()
}

func (g *HTTPGateway) SetSessionIdentity(senderID int64, nimID string) {
	g.senderMu.Lock()
	g.senderID, g.nimID = senderID, strings.TrimSpace(nimID)
	g.senderMu.Unlock()
}

func (g *HTTPGateway) Client() *Client { return g.client }

func (g *HTTPGateway) ListGroups(ctx context.Context) ([]groupmgr.Group, error) {
	infos, err := g.listGroupInfo(ctx)
	if err != nil {
		return nil, gatewayError("list groups", err)
	}
	groups := make([]groupmgr.Group, 0, len(infos))
	for _, info := range infos {
		groups = append(groups, groupmgr.Group{
			AccountID: g.client.creds.ID, GroupID: info.GroupID,
			Name: info.Name, OwnerUserID: info.OwnerUserID,
		})
	}
	return groups, nil
}

func (g *HTTPGateway) ResolveGroupID(ctx context.Context, cloudID string) (int64, error) {
	cloudID = strings.TrimSpace(cloudID)
	if cloudID == "" {
		return 0, invalidGatewayArgument("resolve group", "groupCloudId")
	}
	infos, err := g.listGroupInfo(ctx)
	if err != nil {
		return 0, gatewayError("resolve group", err)
	}
	for _, info := range infos {
		if info.GroupCloudID == cloudID || strconv.FormatInt(info.GroupID, 10) == cloudID {
			return info.GroupID, nil
		}
	}
	return 0, &groupmgr.GatewayError{Op: "resolve group", Code: "GROUP_NOT_FOUND", Message: "groupCloudId is not mapped", Kind: groupmgr.ErrNotFound}
}

func (g *HTTPGateway) ListMembers(ctx context.Context, groupID int64) (groupmgr.MemberRoster, error) {
	if groupID <= 0 {
		return groupmgr.MemberRoster{}, invalidGatewayArgument("list members", "groupId")
	}
	result, err := g.client.call(ctx, http.MethodPost, RouteGroupMembers, GroupMembersRequest{
		GroupID: groupID, Version: GroupProtocolVersion,
	})
	if err != nil {
		return groupmgr.MemberRoster{}, gatewayError("list members", err)
	}
	var data groupMembersData
	if err := decodeResultData(result, &data); err != nil {
		return groupmgr.MemberRoster{}, gatewayError("list members", err)
	}
	members := make([]groupmgr.Member, 0, len(data.Members))
	for _, info := range data.Members {
		if info.UserID <= 0 {
			return groupmgr.MemberRoster{}, gatewayError("list members", fmt.Errorf("群成员资料包含无效的成员 ID"))
		}
		memberGroupID := info.GroupID
		if memberGroupID == 0 {
			memberGroupID = groupID
		}
		members = append(members, groupmgr.Member{
			AccountID: g.client.creds.ID, GroupID: memberGroupID, UserID: info.UserID, NIMID: info.NIMID,
			Nickname: info.Nickname, CardName: info.CardName, AccountState: info.AccountState, Role: memberRole(info.Role),
		})
	}
	roster := groupmgr.MemberRoster{Members: members, ResolvedCount: len(members), Sources: []string{"wangshangliao-http"}}
	if infos, infoErr := g.listGroupInfo(ctx); infoErr == nil {
		for _, info := range infos {
			if info.GroupID == groupID {
				roster.ReportedCount = info.MemberCount
				break
			}
		}
	}
	if nimResult, nimErr := g.client.call(ctx, http.MethodPost, RouteNIMTeamMembers, NIMTeamMembersRequest{GroupID: groupID}); nimErr == nil {
		var nimData nimTeamMembersData
		if decodeResultData(nimResult, &nimData) == nil && nimData.OK {
			byNIM := make(map[string]int, len(roster.Members))
			for index, member := range roster.Members {
				if member.NIMID != "" {
					byNIM[member.NIMID] = index
				}
			}
			for _, nimMember := range nimData.Members {
				if nimMember.NIMID == "" {
					continue
				}
				if index, exists := byNIM[nimMember.NIMID]; exists {
					if wireNameMissing(roster.Members[index].CardName) && !wireNameMissing(nimMember.CardName) {
						roster.Members[index].CardName = nimMember.CardName
					}
					continue
				}
				userID := syntheticNIMUserID(nimMember.NIMID)
				g.senderMu.Lock()
				if nimMember.NIMID == g.nimID && g.senderID > 0 {
					userID = g.senderID
				}
				g.senderMu.Unlock()
				roster.Members = append(roster.Members, groupmgr.Member{
					AccountID: g.client.creds.ID, GroupID: groupID, UserID: userID,
					NIMID: nimMember.NIMID, Nickname: nimMember.CardName, CardName: nimMember.CardName,
					Role: nimMemberRole(nimMember.Type),
				})
			}
			roster.Sources = append(roster.Sources, "nim-team-members")
			roster.ResolvedCount = len(roster.Members)
			if len(nimData.Members) > roster.ReportedCount {
				roster.ReportedCount = len(nimData.Members)
			}
			roster.Complete = len(nimData.Members) > 0 && len(nimData.Members) >= roster.ReportedCount
		}
	}
	if roster.ReportedCount < roster.ResolvedCount {
		roster.ReportedCount = roster.ResolvedCount
	}
	if roster.ReportedCount == roster.ResolvedCount {
		roster.Complete = true
	}
	return roster, nil
}

func (g *HTTPGateway) SendText(ctx context.Context, groupID int64, text string) (string, error) {
	if groupID <= 0 || strings.TrimSpace(text) == "" {
		return "", invalidGatewayArgument("send text", "groupId/text")
	}
	senderID, err := g.resolveSenderID(ctx)
	if err != nil {
		return "", gatewayError("send text", err)
	}
	result, err := g.client.DeliverText(ctx, senderID, groupID, text, SessionGroup)
	if err != nil {
		return "", gatewayError("send text", err)
	}
	var delivery Delivery
	if err := decodeResultData(result, &delivery); err != nil {
		return "", gatewayError("send text", err)
	}
	if delivery.IDServer != "" {
		return delivery.IDServer, nil
	}
	if delivery.IDClient != "" {
		return delivery.IDClient, nil
	}
	return "", gatewayError("send text", fmt.Errorf("发送回执缺少消息 ID"))
}

func (g *HTTPGateway) Recall(ctx context.Context, groupID, senderUserID int64, messageID string) error {
	if groupID <= 0 || senderUserID <= 0 || strings.TrimSpace(messageID) == "" {
		return invalidGatewayArgument("recall", "groupId/userId/msgId")
	}
	infos, err := g.listGroupInfo(ctx)
	if err != nil {
		return gatewayError("recall", err)
	}
	var cloudID string
	for _, info := range infos {
		if info.GroupID == groupID {
			cloudID = info.GroupCloudID
			break
		}
	}
	if cloudID == "" {
		return &groupmgr.GatewayError{
			Op: "recall", Code: "GROUP_NOT_FOUND", Message: "groupId is not mapped to groupCloudId",
			Kind: groupmgr.ErrNotFound,
		}
	}
	return g.action(ctx, "recall", RouteRollbackMessage, RecallRequest{
		GroupCloudID: cloudID, UserID: senderUserID, MessageID: messageID,
	})
}

func (g *HTTPGateway) Mute(ctx context.Context, groupID, userID int64, duration time.Duration) error {
	if groupID <= 0 || userID <= 0 || duration <= 0 {
		return invalidGatewayArgument("mute", "groupId/userId/duration")
	}
	minutes := int(math.Ceil(duration.Minutes()))
	return g.action(ctx, "mute", RouteSetMemberMute, MuteRequest{
		GroupID: groupID, UserID: userID, Minutes: minutes,
	})
}

func (g *HTTPGateway) Unmute(ctx context.Context, groupID, userID int64) error {
	if groupID <= 0 || userID <= 0 {
		return invalidGatewayArgument("unmute", "groupId/userId")
	}
	return g.action(ctx, "unmute", RouteCancelMemberMute, UnmuteRequest{GroupID: groupID, UserID: userID})
}

func (g *HTTPGateway) Rename(ctx context.Context, groupID int64, member groupmgr.MemberRef, nick string) error {
	if groupID <= 0 || !member.Valid() || strings.TrimSpace(nick) == "" {
		return invalidGatewayArgument("rename", "groupId/member/nick")
	}
	if member.UserID > 0 {
		err := g.action(ctx, "rename", RouteSetMemberNickname, RenameRequest{GroupID: groupID, UserID: member.UserID, Nick: nick})
		if err == nil || member.NIMID == "" {
			return err
		}
	}
	return g.action(ctx, "rename nim", RouteNIMUpdateNick, NIMUpdateNickRequest{GroupID: groupID, NIMID: member.NIMID, Nick: nick})
}

func syntheticNIMUserID(nimID string) int64 {
	hasher := fnv.New64a()
	_, _ = hasher.Write([]byte(nimID))
	return -int64(hasher.Sum64()&0x3fffffffffffffff) - 1
}

func nimMemberRole(value string) groupmgr.MemberRole {
	switch strings.ToLower(value) {
	case "owner", "1":
		return groupmgr.RoleOwner
	case "manager", "admin", "2":
		return groupmgr.RoleAdmin
	default:
		return groupmgr.RoleMember
	}
}

func wireNameMissing(value string) bool {
	value = strings.TrimSpace(value)
	return value == "" || value == "1" || value == "."
}

func (g *HTTPGateway) RemoveMember(ctx context.Context, groupID, userID int64) error {
	if groupID <= 0 || userID <= 0 {
		return invalidGatewayArgument("remove member", "groupId/userId")
	}
	return g.action(ctx, "remove member", RouteRemoveGroupMember, RemoveMemberRequest{
		GroupID: groupID, MemberUserIDs: []int64{userID},
	})
}

func (g *HTTPGateway) SetGroupMute(ctx context.Context, groupID int64, muted bool) error {
	if groupID <= 0 {
		return invalidGatewayArgument("set group mute", "groupId")
	}
	mode := GroupMuteDisabled
	if muted {
		mode = GroupMuteMembers
	}
	return g.action(ctx, "set group mute", RouteSetGroupMute, SetGroupMuteRequest{GroupID: groupID, MuteMode: mode})
}

func (g *HTTPGateway) listGroupInfo(ctx context.Context) ([]GroupInfo, error) {
	result, err := g.client.call(ctx, http.MethodPost, RouteGroupList, GroupListRequest{Version: GroupProtocolVersion})
	if err != nil {
		return nil, err
	}
	var data groupListData
	if err := decodeResultData(result, &data); err != nil {
		return nil, err
	}
	groups := make([]GroupInfo, 0, len(data.Owner)+len(data.Member))
	for _, group := range data.Owner {
		group.Relation = GroupRelationOwner
		groups = append(groups, group)
	}
	for _, group := range data.Member {
		group.Relation = GroupRelationMember
		groups = append(groups, group)
	}
	for _, group := range groups {
		if group.GroupID <= 0 {
			return nil, fmt.Errorf("群列表包含无效的群 ID")
		}
	}
	return groups, nil
}

func (g *HTTPGateway) resolveSenderID(ctx context.Context) (int64, error) {
	g.senderMu.Lock()
	defer g.senderMu.Unlock()
	if g.senderID > 0 {
		return g.senderID, nil
	}
	info, _, err := g.client.SessionInfo(ctx)
	if err != nil {
		return 0, err
	}
	if info.SenderID <= 0 {
		return 0, fmt.Errorf("登录会话缺少当前账号 ID")
	}
	g.senderID = info.SenderID
	return g.senderID, nil
}

func (g *HTTPGateway) action(ctx context.Context, op, route string, payload any) error {
	if _, err := g.client.call(ctx, http.MethodPost, route, payload); err != nil {
		return gatewayError(op, err)
	}
	return nil
}

func gatewayError(op string, err error) error {
	if err == nil {
		return nil
	}
	if existing := new(groupmgr.GatewayError); errors.As(err, &existing) {
		return err
	}
	kind := groupmgr.ErrBusiness
	code := "PROTOCOL_ERROR"
	var responseErr *ResponseError
	if errors.As(err, &responseErr) {
		code = fmt.Sprintf("HTTP_%d_CODE_%d_ERRNO_%d", responseErr.HTTPStatus, responseErr.Code, responseErr.Errno)
		switch {
		case errors.Is(responseErr, ErrPermissionDenied):
			kind = groupmgr.ErrPermissionDenied
		case responseErr.HTTPStatus == http.StatusNotFound || responseErr.Code == http.StatusNotFound:
			kind = groupmgr.ErrNotFound
		}
	}
	return &groupmgr.GatewayError{Op: op, Code: code, Message: err.Error(), Kind: kind}
}

func invalidGatewayArgument(op, field string) error {
	return &groupmgr.GatewayError{
		Op: op, Code: "INVALID_ARGUMENT", Message: field + " is invalid", Kind: groupmgr.ErrBusiness,
	}
}

func memberRole(value string) groupmgr.MemberRole {
	normalized := strings.ToUpper(strings.TrimSpace(value))
	switch normalized {
	case "1", "OWNER", "MASTER", "GROUP_OWNER", "GROUP_ROLE_OWNER", "MSG_MASTER":
		return groupmgr.RoleOwner
	case "2", "ADMIN", "ADMINISTRATOR", "GROUP_ADMIN", "GROUP_ROLE_ADMIN", "MSG_ADMIN":
		return groupmgr.RoleAdmin
	default:
		return groupmgr.RoleMember
	}
}

func rawFields(data []byte) (map[string]json.RawMessage, error) {
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(data, &fields); err != nil {
		return nil, err
	}
	return fields, nil
}

func int64Field(fields map[string]json.RawMessage, field string) (int64, error) {
	raw, ok := fields[field]
	if !ok || len(raw) == 0 || string(raw) == "null" || string(raw) == `""` {
		return 0, nil
	}
	var number json.Number
	decoder := json.NewDecoder(strings.NewReader(string(raw)))
	decoder.UseNumber()
	if err := decoder.Decode(&number); err == nil {
		return strconv.ParseInt(number.String(), 10, 64)
	}
	var text string
	if err := json.Unmarshal(raw, &text); err != nil {
		return 0, fmt.Errorf("应为十进制数字 ID")
	}
	return strconv.ParseInt(strings.TrimSpace(text), 10, 64)
}

func firstInt64Field(fields map[string]json.RawMessage, names ...string) (int64, error) {
	for _, name := range names {
		if _, ok := fields[name]; !ok {
			continue
		}
		return int64Field(fields, name)
	}
	return 0, nil
}

func stringIDField(fields map[string]json.RawMessage, field string) (string, error) {
	raw, ok := fields[field]
	if !ok || len(raw) == 0 || string(raw) == "null" || string(raw) == `""` {
		return "", nil
	}
	var text string
	if err := json.Unmarshal(raw, &text); err == nil {
		return strings.TrimSpace(text), nil
	}
	var number json.Number
	decoder := json.NewDecoder(strings.NewReader(string(raw)))
	decoder.UseNumber()
	if err := decoder.Decode(&number); err != nil {
		return "", fmt.Errorf("应为十进制数字 ID")
	}
	return number.String(), nil
}

func firstStringField(fields map[string]json.RawMessage, names ...string) string {
	for _, name := range names {
		raw, ok := fields[name]
		if !ok {
			continue
		}
		var value string
		if json.Unmarshal(raw, &value) == nil && strings.TrimSpace(value) != "" {
			return value
		}
		var number json.Number
		decoder := json.NewDecoder(strings.NewReader(string(raw)))
		decoder.UseNumber()
		if decoder.Decode(&number) == nil {
			return number.String()
		}
	}
	return ""
}

func firstTextField(fields map[string]json.RawMessage, names ...string) string {
	for _, name := range names {
		raw, ok := fields[name]
		if !ok {
			continue
		}
		var value string
		if json.Unmarshal(raw, &value) == nil && strings.TrimSpace(value) != "" {
			return value
		}
	}
	return ""
}

var _ groupmgr.GroupGateway = (*HTTPGateway)(nil)

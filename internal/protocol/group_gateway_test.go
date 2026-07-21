package protocol

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"reflect"
	"testing"
	"time"

	"dh/internal/groupmgr"
)

func TestHTTPGatewayRecoveredGroupContracts(t *testing.T) {
	requests := make(map[string][]map[string]any)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var body map[string]any
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			t.Fatal(err)
		}
		requests[r.URL.Path] = append(requests[r.URL.Path], body)
		w.Header().Set("Content-Type", "application/json")
		switch r.URL.Path {
		case RouteGroupList:
			_, _ = w.Write([]byte(`{"code":0,"errno":0,"msg":"OK","data":{"owner":[{"groupId":"723355","groupCloudId":17656317798,"groupName":"Owner Group","ownerUserId":"7855607"}],"member":[{"groupId":1083990,"groupCloudId":"31968268872","name":"Member Group"}]}}`))
		case RouteGroupMembers:
			_, _ = w.Write([]byte(`{"code":0,"errno":0,"msg":"OK","data":{"groupMemberInfo":[{"groupId":"723355","userId":"7855607","nimId":1571305126,"userNick":"Account Name","groupMemberNick":"Card Name","groupRole":"GROUP_ROLE_ADMIN","accountState":"ACCOUNT_STATE_GOOD"}]}}`))
		case RouteNIMTeamMembers:
			_, _ = w.Write([]byte(`{"code":0,"errno":0,"msg":"OK","data":{"ok":true,"members":[{"nimId":"1571305126","cardName":"Card Name","type":"manager"},{"nimId":"2002","cardName":"NIM only","type":"normal"}]}}`))
		case RouteSendMessage:
			_, _ = w.Write([]byte(`{"code":0,"errno":0,"msg":"OK","data":{"delivered":true,"idClient":"client-1","idServer":"server-1","scene":"team"}}`))
		default:
			_, _ = w.Write([]byte(`{"code":0,"errno":0,"msg":"OK","data":{"v":"0"}}`))
		}
	}))
	defer server.Close()

	gateway := NewHTTPGateway(NewClient(server.URL, Credentials{ID: "ACCOUNT"}))
	gateway.SetSenderID(9001)
	ctx := context.Background()

	groups, err := gateway.ListGroups(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if len(groups) != 2 || groups[0].GroupID != 723355 || groups[0].Name != "Owner Group" ||
		groups[0].OwnerUserID != 7855607 || groups[1].GroupID != 1083990 {
		t.Fatalf("groups = %#v", groups)
	}
	members, err := gateway.ListMembers(ctx, 723355)
	if err != nil {
		t.Fatal(err)
	}
	if len(members.Members) != 2 || members.Members[0].UserID != 7855607 || members.Members[0].Nickname != "Account Name" || members.Members[0].CardName != "Card Name" || members.Members[0].AccountState != "ACCOUNT_STATE_GOOD" ||
		members.Members[0].Role != groupmgr.RoleAdmin || members.Members[1].NIMID != "2002" || members.Members[1].UserID >= 0 {
		t.Fatalf("members = %#v", members)
	}
	messageID, err := gateway.SendText(ctx, 723355, "hello group")
	if err != nil || messageID != "server-1" {
		t.Fatalf("messageID=%q err=%v", messageID, err)
	}
	if err := gateway.Mute(ctx, 723355, 7855607, 10*time.Minute); err != nil {
		t.Fatal(err)
	}
	if err := gateway.Unmute(ctx, 723355, 7855607); err != nil {
		t.Fatal(err)
	}
	if err := gateway.Rename(ctx, 723355, groupmgr.MemberRef{UserID: 7855607}, "New Card"); err != nil {
		t.Fatal(err)
	}
	if err := gateway.RemoveMember(ctx, 723355, 7855607); err != nil {
		t.Fatal(err)
	}
	if err := gateway.SetGroupMute(ctx, 723355, true); err != nil {
		t.Fatal(err)
	}
	if err := gateway.Recall(ctx, 1083990, 7855607, "message-9"); err != nil {
		t.Fatal(err)
	}

	assertJSONBody(t, requests[RouteGroupList][0], map[string]any{"v": "0"})
	assertJSONBody(t, requests[RouteGroupMembers][0], map[string]any{"groupId": float64(723355), "v": "0"})
	assertJSONBody(t, requests[RouteSetMemberMute][0], map[string]any{
		"groupId": float64(723355), "userId": float64(7855607), "min": float64(10),
	})
	assertJSONBody(t, requests[RouteCancelMemberMute][0], map[string]any{
		"groupId": float64(723355), "userId": float64(7855607),
	})
	assertJSONBody(t, requests[RouteSetMemberNickname][0], map[string]any{
		"groupId": float64(723355), "userId": float64(7855607), "nick": "New Card",
	})
	assertJSONBody(t, requests[RouteRemoveGroupMember][0], map[string]any{
		"groupId": float64(723355), "groupMemberIds": []any{float64(7855607)},
	})
	assertJSONBody(t, requests[RouteSetGroupMute][0], map[string]any{
		"groupId": float64(723355), "muteMode": GroupMuteMembers,
	})
	assertJSONBody(t, requests[RouteRollbackMessage][0], map[string]any{
		"groupCloudId": "31968268872", "userId": float64(7855607), "msgId": "message-9",
	})
	var sent struct {
		Message TextMessage `json:"msg"`
	}
	raw, _ := json.Marshal(requests[RouteSendMessage][0])
	if err := json.Unmarshal(raw, &sent); err != nil {
		t.Fatal(err)
	}
	if sent.Message.From.ID != 9001 || sent.Message.To.ID != 723355 ||
		sent.Message.MsgSession != SessionGroup || sent.Message.Content.Data != "hello group" {
		t.Fatalf("sent = %#v", sent.Message)
	}
}

func TestHTTPGatewayStrictResponseErrors(t *testing.T) {
	tests := []struct {
		name   string
		status int
		body   string
		kind   error
	}{
		{name: "http permission", status: http.StatusForbidden, body: `{"code":403,"errno":50,"msg":"denied"}`, kind: groupmgr.ErrPermissionDenied},
		{name: "business code", status: http.StatusOK, body: `{"code":1051,"errno":0,"msg":"failed"}`, kind: groupmgr.ErrBusiness},
		{name: "business errno", status: http.StatusOK, body: `{"code":0,"errno":9,"msg":"failed"}`, kind: groupmgr.ErrBusiness},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
				w.WriteHeader(test.status)
				_, _ = w.Write([]byte(test.body))
			}))
			defer server.Close()
			err := NewHTTPGateway(NewClient(server.URL, Credentials{})).Unmute(context.Background(), 1, 2)
			if !errors.Is(err, test.kind) {
				t.Fatalf("error = %v, want kind %v", err, test.kind)
			}
		})
	}
}

func TestGroupMemberIgnoresNumericNameFlags(t *testing.T) {
	var member GroupMemberInfo
	if err := json.Unmarshal([]byte(`{"groupId":7,"userId":9,"userNick":1,"groupMemberNick":"真实群名片","groupRole":"GROUP_ROLE_MEMBER"}`), &member); err != nil {
		t.Fatal(err)
	}
	if member.Nickname != "真实群名片" || member.CardName != "真实群名片" {
		t.Fatalf("member = %#v", member)
	}
}

func TestNIMRosterReplacesPlaceholderCardName(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		switch r.URL.Path {
		case RouteGroupMembers:
			_, _ = w.Write([]byte(`{"code":0,"errno":0,"msg":"OK","data":{"groupMemberInfo":[{"groupId":7,"userId":9,"nimId":"nim-9","groupMemberNick":"1","groupRole":"GROUP_ROLE_MEMBER"}]}}`))
		case RouteNIMTeamMembers:
			_, _ = w.Write([]byte(`{"code":0,"errno":0,"msg":"OK","data":{"ok":true,"members":[{"nimId":"nim-9","cardName":"真实名称","type":"normal"}]}}`))
		case RouteGroupList:
			_, _ = w.Write([]byte(`{"code":0,"errno":0,"msg":"OK","data":{"owner":[{"groupId":7,"groupCloudId":"77","groupMemberNum":1}],"member":[]}}`))
		default:
			_, _ = w.Write([]byte(`{"code":0,"errno":0,"msg":"OK","data":{}}`))
		}
	}))
	defer server.Close()
	roster, err := NewHTTPGateway(NewClient(server.URL, Credentials{})).ListMembers(context.Background(), 7)
	if err != nil {
		t.Fatal(err)
	}
	if len(roster.Members) != 1 || roster.Members[0].CardName != "真实名称" || !roster.Complete {
		t.Fatalf("roster=%+v", roster)
	}
}

func assertJSONBody(t *testing.T, got, want map[string]any) {
	t.Helper()
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("body = %#v, want %#v", got, want)
	}
}

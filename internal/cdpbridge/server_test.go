package cdpbridge

import (
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestUnknownProtocolRouteIsNotForwarded(t *testing.T) {
	recorder := httptest.NewRecorder()
	request := httptest.NewRequest(http.MethodPost, "/v1/other/action", strings.NewReader(`{}`))
	if err := (&Server{}).route(request.Context(), recorder, request); err != nil {
		t.Fatal(err)
	}
	if recorder.Code != http.StatusNotFound {
		t.Fatalf("status = %d, want 404", recorder.Code)
	}
}

func TestNormalizeMessageForElectronProtobufJS(t *testing.T) {
	message, err := normalizeMessage(map[string]any{
		"created_at": "2026-07-18T10:00:00.123Z",
		"msgDevice":  "Desktop", "msgSession": "MSG_KIND_GROUP",
		"accountType": "ACCOUNT_MEMBER", "msgFormat": "MSG_TEXT",
		"msgRole": "MSG_MINE", "msgRingtone": "MSG_RINGTONE_NONE",
		"appoint": "MSG_APPOINT_NONE",
	})
	if err != nil {
		t.Fatal(err)
	}
	if message["created_at"] != nil {
		t.Fatal("created_at was not removed")
	}
	timestamp := message["createdAt"].(map[string]any)
	want := time.Date(2026, 7, 18, 10, 0, 0, 123000000, time.UTC)
	if timestamp["seconds"] != want.Unix() || timestamp["nanos"] != 123000000 {
		t.Fatalf("timestamp = %#v", timestamp)
	}
	if message["msgDevice"] != 1 || message["msgSession"] != 2 || message["msgFormat"] != 0 {
		t.Fatalf("enum conversion = %#v", message)
	}
}

func TestListenerQueueUsesSequenceAndDurableAck(t *testing.T) {
	listener := listenerExpression()
	for _, fragment := range []string{"limit:5000", "nextSeq:1", "seq:state.nextSeq++", "state.dropped+=overflow", "msgFormat:value.msgFormat", "mentions:value.mentions||value.aite", "quote:value.quote"} {
		if !strings.Contains(listener, fragment) {
			t.Fatalf("listener expression missing %q", fragment)
		}
	}
	poll := pollExpression()
	if !strings.Contains(poll, "state.queue.slice(0,100)") || strings.Contains(poll, "state.queue.splice(0,100)") {
		t.Fatalf("poll must peek without deleting: %s", poll)
	}
	if !strings.Contains(poll, "dropped:state.dropped") || !strings.Contains(poll, "remaining:state.queue.length") {
		t.Fatal("poll does not report dropped/remaining")
	}
	if !strings.Contains(poll, "decoded.aite") || !strings.Contains(poll, "mentions:decoded.aite") {
		t.Fatal("poll does not normalize decoded aite to mentions")
	}
	ack := ackExpression(41)
	for _, fragment := range []string{`"seq":41`, "Number(item.seq)>input.seq", "acked:before-state.queue.length", "remaining:state.queue.length"} {
		if !strings.Contains(ack, fragment) {
			t.Fatalf("ack expression missing %q", fragment)
		}
	}
}

func TestMessageScene(t *testing.T) {
	if messageScene("MSG_KIND_GROUP") != "team" || messageScene(float64(2)) != "team" {
		t.Fatal("group session was not mapped")
	}
	if messageScene("MSG_KIND_P2P") != "p2p" {
		t.Fatal("p2p session was not mapped")
	}
}

func TestNIMMemberExpressionsAreStronglyTyped(t *testing.T) {
	list := nimTeamMembersExpression("TEAM")
	for _, fragment := range []string{"getTeamMembers", "nickInTeam", "item.account", `"teamId":"TEAM"`} {
		if !strings.Contains(list, fragment) {
			t.Fatalf("member expression missing %q", fragment)
		}
	}
	rename := nimUpdateNickExpression("TEAM", "MEMBER", "新名称")
	for _, fragment := range []string{"updateNickInTeam", `"account":"MEMBER"`, `"nickInTeam":"新名称"`} {
		if !strings.Contains(rename, fragment) {
			t.Fatalf("rename expression missing %q", fragment)
		}
	}
}

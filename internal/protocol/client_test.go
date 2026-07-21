package protocol

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestDeliverTextUsesNativeBridgeRoute(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != RouteSendMessage {
			t.Fatalf("path = %s", r.URL.Path)
		}
		var body struct {
			Message TextMessage `json:"msg"`
		}
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			t.Fatal(err)
		}
		if body.Message.MsgSession != SessionP2P || body.Message.Content.Data != "delivered" {
			t.Fatalf("message = %#v", body.Message)
		}
		_, _ = w.Write([]byte(`{"code":0,"msg":"OK","data":{"delivered":true}}`))
	}))
	defer server.Close()

	result, err := NewClient(server.URL, Credentials{}).DeliverText(
		context.Background(), 1001, 2002, "delivered", SessionP2P,
	)
	if err != nil {
		t.Fatal(err)
	}
	if result.Code != 0 {
		t.Fatalf("code = %d", result.Code)
	}
}

func TestParseTeamJoinNotification(t *testing.T) {
	event, ok := ParseTeamJoinNotification(IncomingMessage{
		Scene: "team", To: "31968268872",
		Attach: json.RawMessage(`{"type":"addTeamMembers","team":{"teamId":"31968268872"},"members":["123"]}`),
	})
	if !ok || event.GroupCloudID != "31968268872" {
		t.Fatalf("event=%+v ok=%v", event, ok)
	}
	if _, ok := ParseTeamJoinNotification(IncomingMessage{Scene: "team", To: "31968268872", Attach: json.RawMessage(`{"type":"updateTeam"}`)}); ok {
		t.Fatal("non-join notification was accepted")
	}
}

func TestPollMessagesDecodesBridgeBatch(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != RoutePollMessages {
			t.Fatalf("path = %s", r.URL.Path)
		}
		_, _ = w.Write([]byte(`{"code":0,"errno":0,"msg":"OK","data":{"ok":true,"remaining":1,"dropped":2,"messages":[{"seq":41,"flow":"in","scene":"team","type":"custom","msgFormat":1,"mentions":["1002"],"quote":{"idServer":"quoted"},"decoded":{"from":{"id":1001,"name":"Member"},"to":{"id":2002},"msgSession":2,"msgFormat":0,"content":{"data":"请看群规"},"mentions":["1002"],"quote":{"idServer":"quoted"}}}]}}`))
	}))
	defer server.Close()

	batch, result, err := NewClient(server.URL, Credentials{}).PollMessages(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if result.Code != 0 || !batch.OK || len(batch.Messages) != 1 || batch.Dropped != 2 {
		t.Fatalf("result=%#v batch=%#v", result, batch)
	}
	message := batch.Messages[0]
	if message.Seq != 41 || message.MsgFormat != 1 || len(message.Mentions) == 0 || len(message.Quote) == 0 ||
		message.Decoded == nil || message.Decoded.From.ID != 1001 || message.Decoded.Content.Data != "请看群规" ||
		len(message.Decoded.Mentions) == 0 || len(message.Decoded.Quote) == 0 {
		t.Fatalf("message = %#v", message)
	}
}

func TestAckMessagesUsesSequenceContract(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != RouteAckMessages {
			t.Fatalf("path = %s", r.URL.Path)
		}
		var body map[string]any
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			t.Fatal(err)
		}
		if body["seq"] != float64(41) {
			t.Fatalf("body = %#v", body)
		}
		_, _ = w.Write([]byte(`{"code":0,"errno":0,"msg":"OK","data":{"ok":true,"acked":3,"dropped":2,"remaining":7}}`))
	}))
	defer server.Close()

	ack, _, err := NewClient(server.URL, Credentials{}).AckMessages(context.Background(), 41)
	if err != nil {
		t.Fatal(err)
	}
	if !ack.OK || ack.Acked != 3 || ack.Dropped != 2 || ack.Remaining != 7 {
		t.Fatalf("ack = %#v", ack)
	}
}

func TestSessionInfoDecodesAutoConfiguration(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != RouteSessionInfo {
			t.Fatalf("path = %s", r.URL.Path)
		}
		_, _ = w.Write([]byte(`{"code":0,"errno":0,"msg":"OK","data":{"senderId":1001,"defaultGroupId":2002,"groupCount":3,"nimReady":true}}`))
	}))
	defer server.Close()
	info, _, err := NewClient(server.URL, Credentials{}).SessionInfo(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if info.SenderID != 1001 || info.DefaultGroupID != 2002 || info.GroupCount != 3 || !info.NIMReady {
		t.Fatalf("info = %#v", info)
	}
}

package aiprovider

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"strconv"
	"testing"
	"time"
)

func fixtureRequest() Request {
	return Request{Version: ContractVersion, EventID: "e1", Group: Group{ID: 10}, Member: Member{UserID: 20}, Message: Message{Text: "@DH 你好"}}
}

func TestShouldReply(t *testing.T) {
	for _, text := range []string{"@DH 帮我", "请问 @dh？", "@DH"} {
		if !ShouldReply(text, []string{"小海"}) {
			t.Fatalf("expected trigger for %q", text)
		}
	}
	for _, text := range []string{"大家下午好", "小海，在吗", "为什么这样？", "今天几点开会", "@DHelp 帮我"} {
		if ShouldReply(text, []string{"小海"}) {
			t.Fatalf("unexpected trigger for %q", text)
		}
	}
}

func TestWebhookProvider(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("X-DH-Contract-Version") != ContractVersion {
			t.Error("missing contract header")
		}
		_ = json.NewEncoder(w).Encode(Decision{Reply: "收到", Confidence: 0.9})
	}))
	defer server.Close()
	decision, err := NewWebhook(WebhookConfig{URL: server.URL}).DecideContext(context.Background(), fixtureRequest())
	if err != nil || decision.Reply != "收到" {
		t.Fatalf("decision=%+v err=%v", decision, err)
	}
}

func TestOpenAIProviderAndValidation(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{"choices": []any{map[string]any{"message": map[string]any{"content": `{"actions":[{"type":"mute","groupId":10,"targetUserId":20,"durationMinutes":10,"confidence":0.9}],"confidence":0.9}`}}}})
	}))
	defer server.Close()
	decision, err := NewOpenAI(OpenAIConfig{BaseURL: server.URL, Model: "test"}).DecideContext(context.Background(), fixtureRequest())
	if err != nil || len(decision.Actions) != 1 {
		t.Fatalf("decision=%+v err=%v", decision, err)
	}
	decision.Actions[0].GroupID = 99
	if err := ValidateDecision(fixtureRequest(), decision); err == nil {
		t.Fatal("cross-group action passed validation")
	}
}

func TestNormalizeBaseURL(t *testing.T) {
	tests := map[string]string{
		"https://example.test":                     "https://example.test/v1",
		"https://example.test/":                    "https://example.test/v1",
		"https://example.test/v1":                  "https://example.test/v1",
		"https://example.test/v1/chat/completions": "https://example.test/v1/chat/completions",
	}
	for input, want := range tests {
		if got := normalizeBaseURL(input); got != want {
			t.Fatalf("normalizeBaseURL(%q)=%q, want %q", input, got, want)
		}
	}
}

func TestWebhookTimeout(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(100 * time.Millisecond)
	}))
	defer server.Close()
	_, err := NewWebhook(WebhookConfig{URL: server.URL, Timeout: 10 * time.Millisecond}).DecideContext(context.Background(), fixtureRequest())
	if err == nil {
		t.Fatal("expected timeout")
	}
}

func TestLiveOpenAIProvider(t *testing.T) {
	key := os.Getenv("DH_LIVE_AI_KEY")
	if key == "" {
		t.Skip("DH_LIVE_AI_KEY is not set")
	}
	groupID, err := strconv.ParseInt(os.Getenv("DH_TEST_GROUP_ID"), 10, 64)
	if err != nil || groupID <= 0 {
		t.Fatal("DH_TEST_GROUP_ID must be a positive integer")
	}
	baseURL := os.Getenv("DH_LIVE_AI_BASE_URL")
	model := os.Getenv("DH_LIVE_AI_MODEL")
	request := fixtureRequest()
	request.EventID = "live-ai-config-test"
	request.Group = Group{ID: groupID, Name: os.Getenv("DH_TEST_GROUP_NAME")}
	request.Message = Message{Format: "test", Text: "DH AI configuration test", Time: time.Now().UnixMilli()}
	decision, err := NewOpenAI(OpenAIConfig{BaseURL: baseURL, Model: model, APIKey: key, Timeout: 30 * time.Second}).DecideContext(context.Background(), request)
	if err != nil {
		t.Fatal(err)
	}
	if decision.Reply == "" && decision.Reason == "" && len(decision.Actions) == 0 && len(decision.Tasks) == 0 {
		t.Fatalf("empty decision: %+v", decision)
	}
}

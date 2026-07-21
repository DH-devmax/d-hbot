package cdpbridge

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/coder/websocket"
	"github.com/coder/websocket/wsjson"
)

func TestEvaluateAcceptsLargeDevToolsResponse(t *testing.T) {
	var server *httptest.Server
	server = httptest.NewServer(http.HandlerFunc(func(response http.ResponseWriter, request *http.Request) {
		if request.URL.Path == "/json/list" {
			_ = json.NewEncoder(response).Encode([]map[string]any{{
				"type": "page", "webSocketDebuggerUrl": "ws" + strings.TrimPrefix(server.URL, "http") + "/devtools/page/1",
			}})
			return
		}
		connection, err := websocket.Accept(response, request, nil)
		if err != nil {
			t.Error(err)
			return
		}
		defer connection.Close(websocket.StatusNormalClosure, "done")
		var command map[string]any
		if err := wsjson.Read(request.Context(), connection, &command); err != nil {
			t.Error(err)
			return
		}
		_ = wsjson.Write(request.Context(), connection, map[string]any{
			"id": 1, "result": map[string]any{"result": map[string]any{
				"value": map[string]any{"payload": strings.Repeat("x", 96<<10)},
			}},
		})
	}))
	defer server.Close()

	raw, err := NewCDPClient(server.URL).Evaluate(context.Background(), "({large:true})")
	if err != nil {
		t.Fatal(err)
	}
	if len(raw) < 96<<10 {
		t.Fatalf("response length = %d", len(raw))
	}
}

package cdpbridge

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"time"

	"github.com/coder/websocket"
	"github.com/coder/websocket/wsjson"
)

type CDPClient struct {
	BaseURL string
	HTTP    *http.Client
}

const maxCDPMessageSize = 16 << 20

type devtoolsPage struct {
	Type                 string `json:"type"`
	Title                string `json:"title"`
	URL                  string `json:"url"`
	WebSocketDebuggerURL string `json:"webSocketDebuggerUrl"`
}

type cdpResponse struct {
	ID    int `json:"id"`
	Error *struct {
		Message string `json:"message"`
	} `json:"error,omitempty"`
	Result struct {
		Result struct {
			Value json.RawMessage `json:"value"`
		} `json:"result"`
		ExceptionDetails *struct {
			Text string `json:"text"`
		} `json:"exceptionDetails,omitempty"`
	} `json:"result"`
}

func NewCDPClient(baseURL string) *CDPClient {
	return &CDPClient{
		BaseURL: strings.TrimRight(baseURL, "/"),
		HTTP:    &http.Client{Timeout: 5 * time.Second},
	}
}

func (c *CDPClient) Evaluate(ctx context.Context, expression string) (json.RawMessage, error) {
	websocketURL, err := c.pageURL(ctx)
	if err != nil {
		return nil, err
	}
	connection, _, err := websocket.Dial(ctx, websocketURL, &websocket.DialOptions{
		HTTPHeader: http.Header{"Origin": []string{c.BaseURL}},
	})
	if err != nil {
		return nil, fmt.Errorf("连接旺商聊 DevTools 失败：%w", err)
	}
	defer connection.Close(websocket.StatusNormalClosure, "done")
	connection.SetReadLimit(maxCDPMessageSize)

	request := map[string]any{
		"id": 1, "method": "Runtime.evaluate",
		"params": map[string]any{
			"expression": expression, "awaitPromise": true, "returnByValue": true,
		},
	}
	if err := wsjson.Write(ctx, connection, request); err != nil {
		return nil, fmt.Errorf("写入 DevTools 请求失败：%w", err)
	}
	for {
		var response cdpResponse
		if err := wsjson.Read(ctx, connection, &response); err != nil {
			return nil, fmt.Errorf("读取 DevTools 响应失败：%w", err)
		}
		if response.ID != 1 {
			continue
		}
		if response.Error != nil {
			return nil, fmt.Errorf("DevTools 返回错误：%s", response.Error.Message)
		}
		if response.Result.ExceptionDetails != nil {
			return nil, fmt.Errorf("旺商聊 Electron 执行失败：%s", response.Result.ExceptionDetails.Text)
		}
		if len(response.Result.Result.Value) == 0 {
			return json.RawMessage("null"), nil
		}
		return response.Result.Result.Value, nil
	}
}

func (c *CDPClient) pageURL(ctx context.Context) (string, error) {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, c.BaseURL+"/json/list", nil)
	if err != nil {
		return "", err
	}
	response, err := c.HTTP.Do(request)
	if err != nil {
		return "", fmt.Errorf("读取 DevTools 页面失败：%w", err)
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		return "", fmt.Errorf("DevTools 返回 HTTP %d", response.StatusCode)
	}
	var pages []devtoolsPage
	if err := json.NewDecoder(response.Body).Decode(&pages); err != nil {
		return "", fmt.Errorf("解析 DevTools 页面失败：%w", err)
	}
	bestScore := -1
	bestURL := ""
	for _, page := range pages {
		if page.Type != "page" || page.WebSocketDebuggerURL == "" {
			continue
		}
		value := strings.ToLower(page.Title + " " + page.URL)
		score := 1
		for _, hint := range []string{"wangshangliao", "旺商聊", "wwtalk", "netease", "electron"} {
			if strings.Contains(value, hint) {
				score += 10
			}
		}
		if strings.HasPrefix(strings.TrimSpace(page.URL), "about:") {
			score -= 5
		}
		if score > bestScore {
			bestScore, bestURL = score, page.WebSocketDebuggerURL
		}
	}
	if bestURL != "" {
		return bestURL, nil
	}
	return "", fmt.Errorf("旺商聊 DevTools 页面未就绪")
}

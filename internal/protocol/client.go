package protocol

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

type Credentials struct {
	Token      string `json:"token"`
	GroupToken string `json:"groupToken"`
	JWT        string `json:"jwtToken"`
	ID         string `json:"id"`
}

type Client struct {
	baseURL string
	http    *http.Client
	creds   Credentials
}

type Result struct {
	HTTPStatus int
	ID         json.RawMessage `json:"id"`
	Code       int             `json:"code"`
	Errno      int             `json:"errno"`
	Message    string          `json:"msg"`
	Data       json.RawMessage `json:"data"`
	Raw        []byte
}

var (
	ErrTransport        = errors.New("protocol transport error")
	ErrPermissionDenied = errors.New("protocol permission denied")
	ErrBusiness         = errors.New("protocol business error")
)

type ResponseError struct {
	HTTPStatus int
	Code       int
	Errno      int
	Message    string
	Kind       error
}

func (e *ResponseError) Error() string {
	return fmt.Sprintf("protocol response HTTP %d code %d errno %d: %s", e.HTTPStatus, e.Code, e.Errno, e.Message)
}

func (e *ResponseError) Unwrap() error {
	if e.Kind == nil {
		return ErrBusiness
	}
	return e.Kind
}

func NewClient(baseURL string, creds Credentials) *Client {
	if strings.TrimSpace(baseURL) == "" {
		baseURL = DefaultBaseURL
	}
	return &Client{
		baseURL: strings.TrimRight(baseURL, "/"), creds: creds,
		http: &http.Client{Timeout: 12 * time.Second},
	}
}

func (c *Client) SetCredentials(creds Credentials) { c.creds = creds }

func (c *Client) Ping(ctx context.Context) (Result, error) {
	return c.call(ctx, http.MethodGet, RoutePing, nil)
}

func (c *Client) DeliverText(ctx context.Context, fromID, toID int64, text, session string) (Result, error) {
	message := NewTextMessage(fromID, toID, text, session, time.Now())
	return c.call(ctx, http.MethodPost, RouteSendMessage, map[string]any{"msg": message})
}

func (c *Client) ListenMessages(ctx context.Context) (ListenerState, Result, error) {
	result, err := c.call(ctx, http.MethodPost, RouteListenMessages, map[string]any{})
	if err != nil {
		return ListenerState{}, result, err
	}
	var state ListenerState
	if err := decodeResultData(result, &state); err != nil {
		return ListenerState{}, result, err
	}
	return state, result, nil
}

func (c *Client) PollMessages(ctx context.Context) (MessageBatch, Result, error) {
	result, err := c.call(ctx, http.MethodPost, RoutePollMessages, map[string]any{})
	if err != nil {
		return MessageBatch{}, result, err
	}
	var batch MessageBatch
	if err := decodeResultData(result, &batch); err != nil {
		return MessageBatch{}, result, err
	}
	return batch, result, nil
}

func (c *Client) PeekMessages(ctx context.Context) (MessageBatch, Result, error) {
	result, err := c.call(ctx, http.MethodPost, RoutePeekMessages, map[string]any{})
	if err != nil {
		return MessageBatch{}, result, err
	}
	var batch MessageBatch
	if err := decodeResultData(result, &batch); err != nil {
		return MessageBatch{}, result, err
	}
	return batch, result, nil
}

// AckMessages removes all queued messages through seq after the caller has
// durably stored them.
func (c *Client) AckMessages(ctx context.Context, seq uint64) (MessageAck, Result, error) {
	result, err := c.call(ctx, http.MethodPost, RouteAckMessages, map[string]any{"seq": seq})
	if err != nil {
		return MessageAck{}, result, err
	}
	var ack MessageAck
	if err := decodeResultData(result, &ack); err != nil {
		return MessageAck{}, result, err
	}
	return ack, result, nil
}

func (c *Client) SessionInfo(ctx context.Context) (SessionInfo, Result, error) {
	result, err := c.call(ctx, http.MethodPost, RouteSessionInfo, map[string]any{})
	if err != nil {
		return SessionInfo{}, result, err
	}
	var info SessionInfo
	if err := decodeResultData(result, &info); err != nil {
		return SessionInfo{}, result, err
	}
	return info, result, nil
}

func (c *Client) call(ctx context.Context, method, route string, payload any) (Result, error) {
	var body io.Reader
	if payload != nil {
		encoded, err := json.Marshal(payload)
		if err != nil {
			return Result{}, fmt.Errorf("编码协议请求失败：%w", err)
		}
		body = bytes.NewReader(encoded)
	}
	req, err := http.NewRequestWithContext(ctx, method, c.baseURL+route, body)
	if err != nil {
		return Result{}, fmt.Errorf("创建协议请求失败：%w", err)
	}
	if payload != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	setHeader(req, "X-Token", c.creds.Token)
	setHeader(req, "X-Group-Token", c.creds.GroupToken)
	setHeader(req, "X-jwt", c.creds.JWT)
	setHeader(req, "X-id", c.creds.ID)
	resp, err := c.http.Do(req)
	if err != nil {
		return Result{}, fmt.Errorf("请求协议路由 %s 失败：%w", route, err)
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(resp.Body, 8<<20))
	if err != nil {
		return Result{}, fmt.Errorf("读取协议响应失败：%w", err)
	}
	result := Result{HTTPStatus: resp.StatusCode, Raw: raw}
	if err := json.Unmarshal(raw, &result); err != nil {
		return result, fmt.Errorf("解析协议响应失败：%w", err)
	}
	if err := validateResult(result); err != nil {
		return result, err
	}
	return result, nil
}

func setHeader(req *http.Request, key, value string) {
	if value != "" {
		req.Header.Set(key, value)
	}
}

func decodeResultData(result Result, destination any) error {
	if err := validateResult(result); err != nil {
		return err
	}
	if len(result.Data) == 0 || string(result.Data) == "null" {
		return fmt.Errorf("协议响应数据为空")
	}
	if err := json.Unmarshal(result.Data, destination); err != nil {
		return fmt.Errorf("解析协议数据失败：%w", err)
	}
	return nil
}

func validateResult(result Result) error {
	kind := error(nil)
	switch {
	case result.HTTPStatus < http.StatusOK || result.HTTPStatus >= http.StatusMultipleChoices:
		kind = ErrTransport
	case result.Code != 0 || result.Errno != 0:
		kind = ErrBusiness
	default:
		return nil
	}
	if result.HTTPStatus == http.StatusUnauthorized || result.HTTPStatus == http.StatusForbidden ||
		result.Code == http.StatusUnauthorized || result.Code == http.StatusForbidden ||
		result.Errno == http.StatusUnauthorized || result.Errno == http.StatusForbidden {
		kind = ErrPermissionDenied
	}
	return &ResponseError{
		HTTPStatus: result.HTTPStatus, Code: result.Code, Errno: result.Errno,
		Message: result.Message, Kind: kind,
	}
}

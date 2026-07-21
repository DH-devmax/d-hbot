package aiprovider

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

type OpenAIConfig struct {
	BaseURL string
	Model   string
	APIKey  string
	Timeout time.Duration
}

type OpenAIProvider struct {
	config OpenAIConfig
	client *http.Client
}

func NewOpenAI(config OpenAIConfig) *OpenAIProvider {
	if config.Timeout <= 0 {
		config.Timeout = 15 * time.Second
	}
	return &OpenAIProvider{config: config, client: &http.Client{Timeout: config.Timeout}}
}

func (provider *OpenAIProvider) Decide(ctx context.Context, request Request) (Decision, error) {
	if err := ValidateRequest(request); err != nil {
		return Decision{}, err
	}
	requestJSON, err := json.Marshal(request)
	if err != nil {
		return Decision{}, err
	}
	body, err := json.Marshal(map[string]any{
		"model":           provider.config.Model,
		"temperature":     0.2,
		"response_format": map[string]string{"type": "json_object"},
		"messages": []map[string]string{
			{"role": "system", "content": DefaultPersona + "\n\nReturn one JSON object containing only reply, actions, tasks, confidence, and reason. For advertisement, abuse, scam, or another moderation classification, put its stable category and score in an action's category and confidence fields. Do not invent group IDs, user IDs, or message IDs."},
			{"role": "user", "content": string(requestJSON)},
		},
	})
	if err != nil {
		return Decision{}, err
	}
	url := normalizeBaseURL(provider.config.BaseURL)
	if !strings.HasSuffix(url, "/chat/completions") {
		url += "/chat/completions"
	}
	httpRequest, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(body))
	if err != nil {
		return Decision{}, err
	}
	httpRequest.Header.Set("Content-Type", "application/json")
	if provider.config.APIKey != "" {
		httpRequest.Header.Set("Authorization", "Bearer "+provider.config.APIKey)
	}
	response, err := provider.client.Do(httpRequest)
	if err != nil {
		return Decision{}, fmt.Errorf("AI 请求失败：%w", err)
	}
	defer response.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(response.Body, 4<<20))
	if err != nil {
		return Decision{}, err
	}
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		return Decision{}, fmt.Errorf("AI 请求返回 HTTP %d：%s", response.StatusCode, strings.TrimSpace(string(raw)))
	}
	var envelope struct {
		Choices []struct {
			Message struct {
				Content string `json:"content"`
			} `json:"message"`
		} `json:"choices"`
	}
	if err := json.Unmarshal(raw, &envelope); err != nil || len(envelope.Choices) == 0 {
		return Decision{}, fmt.Errorf("AI 响应中没有可用结果")
	}
	var decision Decision
	if err := decodeDecision([]byte(envelope.Choices[0].Message.Content), &decision); err != nil {
		return Decision{}, err
	}
	if err := ValidateDecision(request, decision); err != nil {
		return Decision{}, err
	}
	return decision, nil
}

func normalizeBaseURL(raw string) string {
	value := strings.TrimRight(strings.TrimSpace(raw), "/")
	if value == "" || strings.HasSuffix(value, "/chat/completions") {
		return value
	}
	if !strings.HasSuffix(value, "/v1") {
		value += "/v1"
	}
	return value
}

func (provider *OpenAIProvider) DecideContext(ctx context.Context, request Request) (Decision, error) {
	return provider.Decide(ctx, request)
}

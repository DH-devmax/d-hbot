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

type WebhookConfig struct {
	URL     string
	Token   string
	Timeout time.Duration
}

type WebhookProvider struct {
	config WebhookConfig
	client *http.Client
}

func NewWebhook(config WebhookConfig) *WebhookProvider {
	if config.Timeout <= 0 {
		config.Timeout = 15 * time.Second
	}
	return &WebhookProvider{config: config, client: &http.Client{Timeout: config.Timeout}}
}

func (provider *WebhookProvider) Decide(ctx context.Context, request Request) (Decision, error) {
	if err := ValidateRequest(request); err != nil {
		return Decision{}, err
	}
	body, err := json.Marshal(request)
	if err != nil {
		return Decision{}, err
	}
	httpRequest, err := http.NewRequestWithContext(ctx, http.MethodPost, provider.config.URL, bytes.NewReader(body))
	if err != nil {
		return Decision{}, err
	}
	httpRequest.Header.Set("Content-Type", "application/json")
	httpRequest.Header.Set("X-DH-Contract-Version", ContractVersion)
	if provider.config.Token != "" {
		httpRequest.Header.Set("Authorization", "Bearer "+provider.config.Token)
	}
	response, err := provider.client.Do(httpRequest)
	if err != nil {
		return Decision{}, fmt.Errorf("Webhook 请求失败：%w", err)
	}
	defer response.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(response.Body, 4<<20))
	if err != nil {
		return Decision{}, err
	}
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		return Decision{}, fmt.Errorf("Webhook 返回 HTTP %d：%s", response.StatusCode, strings.TrimSpace(string(raw)))
	}
	var decision Decision
	if err := decodeDecision(raw, &decision); err != nil {
		return Decision{}, err
	}
	if err := ValidateDecision(request, decision); err != nil {
		return Decision{}, err
	}
	return decision, nil
}

func (provider *WebhookProvider) DecideContext(ctx context.Context, request Request) (Decision, error) {
	return provider.Decide(ctx, request)
}

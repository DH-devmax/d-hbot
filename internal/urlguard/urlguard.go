package urlguard

import (
	"fmt"
	"net"
	"net/url"
	"strings"
)

// ValidateLoopback accepts only local HTTP(S) services such as the embedded
// bridge and WangShangLiao DevTools endpoint.
func ValidateLoopback(raw string) error {
	u, err := parse(raw)
	if err != nil {
		return err
	}
	if !isLoopback(u.Hostname()) {
		return fmt.Errorf("地址必须指向本机回环接口")
	}
	return nil
}

// ValidateOutbound accepts HTTPS for remote services and HTTP(S) for local
// services, which keeps local model servers useful without allowing plaintext
// credentials to leave the machine.
func ValidateOutbound(raw string) error {
	u, err := parse(raw)
	if err != nil {
		return err
	}
	if !isLoopback(u.Hostname()) && !strings.EqualFold(u.Scheme, "https") {
		return fmt.Errorf("远程 AI 服务必须使用 HTTPS")
	}
	return nil
}

func parse(raw string) (*url.URL, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return nil, fmt.Errorf("地址不能为空")
	}
	u, err := url.ParseRequestURI(raw)
	if err != nil || u.Scheme == "" || u.Host == "" {
		return nil, fmt.Errorf("地址格式不正确")
	}
	if u.User != nil || (u.Scheme != "http" && u.Scheme != "https") {
		return nil, fmt.Errorf("地址格式或协议不受支持")
	}
	return u, nil
}

func isLoopback(host string) bool {
	host = strings.TrimSpace(strings.ToLower(host))
	if host == "localhost" {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}

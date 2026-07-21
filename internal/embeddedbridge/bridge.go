package embeddedbridge

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"strings"
	"sync"
	"time"

	"dh/internal/cdpbridge"
)

const (
	DefaultAddress     = "127.0.0.1:51235"
	DefaultDevToolsURL = "http://127.0.0.1:9222"
)

type Options struct {
	Address      string
	DevToolsURL  string
	ProbeTimeout time.Duration
	Handler      http.Handler
	HTTPClient   *http.Client
}

type Service struct {
	URL    string
	Reused bool

	server *http.Server
	done   chan struct{}

	closeOnce sync.Once
	mu        sync.Mutex
	err       error
	closeErr  error
}

// Start starts the loopback bridge in the current process. If another DH
// bridge already owns the address, Start reuses it after validating /ping.
func Start(ctx context.Context, options Options) (*Service, error) {
	if ctx == nil {
		ctx = context.Background()
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	options = defaultOptions(options)
	if err := validateAddress(options.Address); err != nil {
		return nil, err
	}

	listener, listenErr := net.Listen("tcp", options.Address)
	if listenErr != nil {
		if probeErr := probeExisting(ctx, options); probeErr == nil {
			service := &Service{
				URL: "http://" + options.Address, Reused: true, done: make(chan struct{}),
			}
			go service.closeOnContext(ctx)
			return service, nil
		} else {
			return nil, fmt.Errorf("本地桥监听 %s 失败：%w；已有桥检测结果：%v", options.Address, listenErr, probeErr)
		}
	}

	handler := options.Handler
	if handler == nil {
		handler = cdpbridge.NewServer(options.DevToolsURL).Handler()
	}
	service := &Service{
		URL: "http://" + listener.Addr().String(), done: make(chan struct{}),
		server: &http.Server{
			Handler:           handler,
			ReadHeaderTimeout: 5 * time.Second,
			IdleTimeout:       30 * time.Second,
		},
	}
	go service.serve(listener)
	go service.closeOnContext(ctx)
	return service, nil
}

func (s *Service) Done() <-chan struct{} { return s.done }

func (s *Service) Wait() error {
	<-s.done
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.err
}

func (s *Service) Close() error {
	s.closeOnce.Do(func() {
		if s.server == nil {
			close(s.done)
			return
		}
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		if err := s.server.Shutdown(ctx); err != nil {
			s.closeErr = err
			_ = s.server.Close()
		}
	})
	<-s.done
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.closeErr
}

func (s *Service) serve(listener net.Listener) {
	err := s.server.Serve(listener)
	if err != nil && !errors.Is(err, http.ErrServerClosed) {
		s.mu.Lock()
		s.err = err
		s.mu.Unlock()
	}
	close(s.done)
}

func (s *Service) closeOnContext(ctx context.Context) {
	select {
	case <-ctx.Done():
		_ = s.Close()
	case <-s.done:
	}
}

func defaultOptions(options Options) Options {
	if strings.TrimSpace(options.Address) == "" {
		options.Address = DefaultAddress
	}
	if strings.TrimSpace(options.DevToolsURL) == "" {
		options.DevToolsURL = DefaultDevToolsURL
	}
	if options.ProbeTimeout <= 0 {
		options.ProbeTimeout = time.Second
	}
	if options.HTTPClient == nil {
		options.HTTPClient = &http.Client{Timeout: 250 * time.Millisecond}
	}
	return options
}

func validateAddress(address string) error {
	host, _, err := net.SplitHostPort(address)
	if err != nil {
		return fmt.Errorf("本地桥地址 %q 无效：%w", address, err)
	}
	if strings.EqualFold(host, "localhost") {
		return nil
	}
	ip := net.ParseIP(host)
	if ip == nil || !ip.IsLoopback() {
		return fmt.Errorf("本地桥地址必须使用回环地址：%s", address)
	}
	return nil
}

func probeExisting(ctx context.Context, options Options) error {
	deadline := time.Now().Add(options.ProbeTimeout)
	var lastErr error
	for {
		if err := ctx.Err(); err != nil {
			return err
		}
		if err := probeOnce(ctx, options.HTTPClient, "http://"+options.Address+"/ping"); err == nil {
			return nil
		} else {
			lastErr = err
		}
		if time.Now().After(deadline) {
			return lastErr
		}
		timer := time.NewTimer(50 * time.Millisecond)
		select {
		case <-ctx.Done():
			timer.Stop()
			return ctx.Err()
		case <-timer.C:
		}
	}
}

func probeOnce(ctx context.Context, client *http.Client, url string) error {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}
	response, err := client.Do(request)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		_, _ = io.Copy(io.Discard, io.LimitReader(response.Body, 4<<10))
		return fmt.Errorf("本地桥检测返回 HTTP %d", response.StatusCode)
	}
	var envelope struct {
		Code  int `json:"code"`
		Errno int `json:"errno"`
		Data  struct {
			Bridge string `json:"bridge"`
		} `json:"data"`
	}
	if err := json.NewDecoder(io.LimitReader(response.Body, 64<<10)).Decode(&envelope); err != nil {
		return fmt.Errorf("解析本地桥检测响应失败：%w", err)
	}
	if envelope.Code != 0 || envelope.Errno != 0 || envelope.Data.Bridge != "native" {
		return fmt.Errorf("目标端口不是 DH 本地桥")
	}
	return nil
}

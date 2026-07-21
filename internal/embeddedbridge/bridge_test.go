package embeddedbridge

import (
	"context"
	"encoding/json"
	"net"
	"net/http"
	"testing"
	"time"
)

func TestStartOwnsServerAndContextStopsIt(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	service, err := Start(ctx, Options{
		Address: "127.0.0.1:0",
		Handler: nativePingHandler(),
	})
	if err != nil {
		t.Fatal(err)
	}
	if service.Reused {
		t.Fatal("new listener was marked reused")
	}
	assertNativePing(t, service.URL+"/ping")

	cancel()
	select {
	case <-service.Done():
	case <-time.After(3 * time.Second):
		t.Fatal("service did not close after context cancellation")
	}
	if err := service.Wait(); err != nil {
		t.Fatalf("wait: %v", err)
	}
}

func TestStartReusesExistingNativeBridge(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	server := &http.Server{Handler: nativePingHandler()}
	go func() { _ = server.Serve(listener) }()
	defer server.Close()

	service, err := Start(context.Background(), Options{
		Address: listener.Addr().String(), ProbeTimeout: 300 * time.Millisecond,
	})
	if err != nil {
		t.Fatal(err)
	}
	if !service.Reused {
		t.Fatal("occupied native bridge was not reused")
	}
	if err := service.Close(); err != nil {
		t.Fatal(err)
	}
	assertNativePing(t, "http://"+listener.Addr().String()+"/ping")
}

func TestStartRejectsUnrelatedOccupiedPort(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	server := &http.Server{Handler: http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{"ok": true})
	})}
	go func() { _ = server.Serve(listener) }()
	defer server.Close()

	service, err := Start(context.Background(), Options{
		Address: listener.Addr().String(), ProbeTimeout: 80 * time.Millisecond,
		HTTPClient: &http.Client{Timeout: 40 * time.Millisecond},
	})
	if err == nil || service != nil {
		t.Fatalf("service=%#v err=%v", service, err)
	}
}

func nativePingHandler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, request *http.Request) {
		if request.URL.Path != "/ping" {
			http.NotFound(w, request)
			return
		}
		w.Header().Set("Content-Type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{
			"code": 0, "errno": 0, "msg": "OK", "data": map[string]any{"bridge": "native"},
		})
	})
}

func assertNativePing(t *testing.T, url string) {
	t.Helper()
	response, err := http.Get(url)
	if err != nil {
		t.Fatal(err)
	}
	defer response.Body.Close()
	if response.StatusCode != http.StatusOK {
		t.Fatalf("ping HTTP %d", response.StatusCode)
	}
}

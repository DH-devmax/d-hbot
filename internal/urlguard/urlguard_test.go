package urlguard

import "testing"

func TestValidateLoopback(t *testing.T) {
	for _, value := range []string{"http://127.0.0.1:9222", "http://localhost:51235", "https://[::1]:9222"} {
		if err := ValidateLoopback(value); err != nil {
			t.Fatalf("%s: %v", value, err)
		}
	}
	for _, value := range []string{"https://example.com", "http://192.168.1.10:9222", "file:///tmp/dh"} {
		if err := ValidateLoopback(value); err == nil {
			t.Fatalf("expected loopback rejection for %s", value)
		}
	}
}

func TestValidateOutbound(t *testing.T) {
	for _, value := range []string{"https://example.com/v1", "http://127.0.0.1:8080", "http://localhost:9000"} {
		if err := ValidateOutbound(value); err != nil {
			t.Fatalf("%s: %v", value, err)
		}
	}
	if err := ValidateOutbound("http://example.com/v1"); err == nil {
		t.Fatal("expected plaintext remote rejection")
	}
}

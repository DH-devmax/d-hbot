package diagnostics

import (
	"strings"
	"testing"
)

func TestRedact(t *testing.T) {
	input := `apiKey=secret Token: bearer-value Authorization="abc" Cookie=session-id ordinary=visible`
	result := Redact(input)
	for _, secret := range []string{"secret", "bearer-value", "abc", "session-id"} {
		if strings.Contains(result, secret) {
			t.Fatalf("secret %q remained in %q", secret, result)
		}
	}
	if !strings.Contains(result, "ordinary=visible") {
		t.Fatalf("ordinary text changed: %q", result)
	}
}

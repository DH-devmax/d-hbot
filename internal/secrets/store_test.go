package secrets

import (
	"path/filepath"
	"testing"
)

func TestStoreRoundTrip(t *testing.T) {
	store := Store{Path: filepath.Join(t.TempDir(), "secrets.dat")}
	want := map[string]string{"ai.default.apiKey": "secret", "webhook.token": "token"}
	if err := store.Save(want); err != nil {
		t.Fatal(err)
	}
	got, err := store.Load()
	if err != nil {
		t.Fatal(err)
	}
	if got["ai.default.apiKey"] != "secret" || got["webhook.token"] != "token" {
		t.Fatalf("unexpected secrets: %#v", got)
	}
}

package migration

import (
	"os"
	"path/filepath"
	"testing"
)

func TestLegacyConnectionAndRemove(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	if err := os.WriteFile(path, []byte(`{"settings":{"endpoint":"http://127.0.0.1:51235","defaultGroupId":1143980},"players":[{"id":"discard"}]}`), 0o600); err != nil {
		t.Fatal(err)
	}
	connection, exists, err := ReadLegacy(path)
	if err != nil || !exists || connection.DefaultGroupID != 1143980 {
		t.Fatalf("connection=%+v exists=%v err=%v", connection, exists, err)
	}
	if err := RemoveLegacy(path); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatalf("legacy state remains: %v", err)
	}
}

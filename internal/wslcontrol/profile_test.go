package wslcontrol

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestPatchProfileScript(t *testing.T) {
	source := []byte("before;" + legacyPartitionCode + ";after")
	patched, changed, err := patchProfileScript(source)
	if err != nil || !changed {
		t.Fatalf("changed=%v err=%v", changed, err)
	}
	if strings.Count(string(patched), fixedPartitionCode) != 1 || strings.Contains(string(patched), legacyPartitionCode) {
		t.Fatalf("unexpected patch result: %s", patched)
	}

	again, changed, err := patchProfileScript(patched)
	if err != nil || changed || string(again) != string(patched) {
		t.Fatalf("idempotent patch changed=%v err=%v", changed, err)
	}
}

func TestPatchProfileScriptRejectsUnknownVersion(t *testing.T) {
	if _, _, err := patchProfileScript([]byte("persistFlag='other'")); err == nil {
		t.Fatal("unknown script version was accepted")
	}
}

func TestMigrateLatestPartition(t *testing.T) {
	root := t.TempDir()
	partitions := filepath.Join(root, "Partitions")
	if err := os.MkdirAll(partitions, 0o755); err != nil {
		t.Fatal(err)
	}
	older := filepath.Join(partitions, "1700000000000")
	newer := filepath.Join(partitions, "1800000000000")
	for _, path := range []string{older, newer, filepath.Join(partitions, "other")} {
		if err := os.Mkdir(path, 0o755); err != nil {
			t.Fatal(err)
		}
	}
	oldTime := time.Now().Add(-time.Hour)
	if err := os.Chtimes(older, oldTime, oldTime); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(newer, "login-state"), []byte("fixture"), 0o600); err != nil {
		t.Fatal(err)
	}
	if !migrateLatestPartition(root, defaultProfileID) {
		t.Fatal("latest partition was not migrated")
	}
	if data, err := os.ReadFile(filepath.Join(partitions, defaultProfileID, "login-state")); err != nil || string(data) != "fixture" {
		t.Fatalf("migrated data=%q err=%v", data, err)
	}
	if migrateLatestPartition(root, defaultProfileID) {
		t.Fatal("existing fixed partition was replaced")
	}
}

func TestMatchingBackupAndRestore(t *testing.T) {
	root := t.TempDir()
	backupDir := filepath.Join(root, "wsl-backups")
	if err := os.MkdirAll(backupDir, 0o755); err != nil {
		t.Fatal(err)
	}
	original := []byte("before;" + legacyPartitionCode + ";after")
	patched, _, err := patchProfileScript(original)
	if err != nil {
		t.Fatal(err)
	}
	backup := filepath.Join(backupDir, "wangshangliao-fixture.index.js")
	if err := os.WriteFile(backup, original, 0o600); err != nil {
		t.Fatal(err)
	}
	if got := matchingProfileBackup(backupDir, patched); got != backup {
		t.Fatalf("matching backup=%q, want %q", got, backup)
	}
	script := filepath.Join(root, "index.js")
	if err := os.WriteFile(script, patched, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := restoreProfileScript(script, backup); err != nil {
		t.Fatal(err)
	}
	if got, err := os.ReadFile(script); err != nil || string(got) != string(original) {
		t.Fatalf("restored=%q err=%v", got, err)
	}
}

func TestRestoreRejectsInvalidBackup(t *testing.T) {
	root := t.TempDir()
	script := filepath.Join(root, "index.js")
	backup := filepath.Join(root, "bad.index.js")
	if err := os.WriteFile(script, []byte(fixedPartitionCode), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(backup, []byte("not a supported script"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := restoreProfileScript(script, backup); err == nil {
		t.Fatal("invalid backup was restored")
	}
}

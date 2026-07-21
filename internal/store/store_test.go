package store

import (
	"context"
	"database/sql"
	"os"
	"path/filepath"
	"testing"
	"time"

	"dh/internal/groupmgr"
)

func openTestStore(t *testing.T) *Store {
	t.Helper()
	s, err := Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

func TestMigrateIsIdempotentAndEnablesPragmas(t *testing.T) {
	s := openTestStore(t)
	ctx := context.Background()
	if err := s.Migrate(ctx); err != nil {
		t.Fatal(err)
	}
	var foreign int
	if err := s.DB.QueryRowContext(ctx, "PRAGMA foreign_keys").Scan(&foreign); err != nil || foreign != 1 {
		t.Fatalf("foreign_keys=%d err=%v", foreign, err)
	}
	var journal string
	if err := s.DB.QueryRowContext(ctx, "PRAGMA journal_mode").Scan(&journal); err != nil || journal != "wal" {
		t.Fatalf("journal=%q err=%v", journal, err)
	}
	for _, table := range []string{"groups", "members", "messages", "daily_summaries", "rules", "knowledge", "tasks", "actions", "audit_events", "app_settings", "group_card_settings", "card_rename_jobs", "group_schedules", "group_schedule_groups", "schedule_runs"} {
		var name string
		if err := s.DB.QueryRowContext(ctx, `SELECT name FROM sqlite_master WHERE type='table' AND name=?`, table).Scan(&name); err != nil {
			t.Fatalf("table %s: %v", table, err)
		}
	}
	var version int
	if err := s.DB.QueryRowContext(ctx, "PRAGMA user_version").Scan(&version); err != nil || version != schemaVersion {
		t.Fatalf("user_version=%d err=%v", version, err)
	}
	for _, column := range []struct{ table, name string }{{"groups", "task_reminders_enabled"}, {"tasks", "reminded_at"}, {"members", "account_state"}, {"schedule_runs", "attempts"}, {"schedule_runs", "next_retry_at"}} {
		exists, err := s.hasColumn(ctx, column.table, column.name)
		if err != nil || !exists {
			t.Fatalf("column %s.%s exists=%v err=%v", column.table, column.name, exists, err)
		}
	}
}

func TestOpenChecksIntegrityAndBacksUpBeforeMigration(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "dh.db")
	s, err := Open(path)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("PRAGMA user_version = 5"); err != nil {
		t.Fatal(err)
	}
	if err := s.Close(); err != nil {
		t.Fatal(err)
	}
	s, err = Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	backups, err := filepath.Glob(filepath.Join(dir, "backups", "dh-*.db"))
	if err != nil || len(backups) != 1 {
		t.Fatalf("backups=%v err=%v", backups, err)
	}
	if err := s.CheckIntegrity(context.Background()); err != nil {
		t.Fatal(err)
	}
	for i := 0; i < 4; i++ {
		if _, err := s.BackupBeforeMigration(context.Background(), 3); err != nil {
			t.Fatal(err)
		}
	}
	backups, err = filepath.Glob(filepath.Join(dir, "backups", "dh-*.db"))
	if err != nil || len(backups) != 3 {
		t.Fatalf("rotated backups=%v err=%v", backups, err)
	}
}

func TestOpenPreservesCorruptDatabase(t *testing.T) {
	path := filepath.Join(t.TempDir(), "dh.db")
	original := []byte("not a sqlite database")
	if err := os.WriteFile(path, original, 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := Open(path); err == nil {
		t.Fatal("expected corrupt database error")
	}
	raw, err := os.ReadFile(path)
	if err != nil || string(raw) != string(original) {
		t.Fatalf("database changed: raw=%q err=%v", raw, err)
	}
}

func TestMigrateV2MembersBeforeCreatingManagedNameIndex(t *testing.T) {
	path := filepath.Join(t.TempDir(), "v2.db")
	db, err := sql.Open("sqlite", path)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	oldSchema := `
CREATE TABLE groups (account_id TEXT NOT NULL DEFAULT '',group_id INTEGER NOT NULL,name TEXT NOT NULL DEFAULT '',owner_user_id INTEGER NOT NULL DEFAULT 0,enabled INTEGER NOT NULL DEFAULT 0,ai_enabled INTEGER NOT NULL DEFAULT 0,moderation_enabled INTEGER NOT NULL DEFAULT 0,manual_takeover INTEGER NOT NULL DEFAULT 0,task_reminders_enabled INTEGER NOT NULL DEFAULT 1,welcome_message TEXT NOT NULL DEFAULT '',summary_schedule TEXT NOT NULL DEFAULT '',created_at TEXT NOT NULL,updated_at TEXT NOT NULL,synced_at TEXT NOT NULL DEFAULT '',PRIMARY KEY(account_id,group_id));
CREATE TABLE members (account_id TEXT NOT NULL DEFAULT '',group_id INTEGER NOT NULL,user_id INTEGER NOT NULL,nickname TEXT NOT NULL DEFAULT '',card_name TEXT NOT NULL DEFAULT '',role TEXT NOT NULL DEFAULT 'member',blacklisted INTEGER NOT NULL DEFAULT 0,locked_card_name TEXT NOT NULL DEFAULT '',rename_violations INTEGER NOT NULL DEFAULT 0,joined_at TEXT NOT NULL DEFAULT '',last_seen_at TEXT NOT NULL DEFAULT '',updated_at TEXT NOT NULL,PRIMARY KEY(account_id,group_id,user_id));
PRAGMA user_version=2;`
	if _, err := db.Exec(oldSchema); err != nil {
		t.Fatal(err)
	}
	s := New(db)
	if err := s.Migrate(context.Background()); err != nil {
		t.Fatal(err)
	}
	for _, column := range []string{"nim_id", "account_state", "managed_card_name", "card_suffix", "present", "card_status"} {
		exists, err := s.hasColumn(context.Background(), "members", column)
		if err != nil || !exists {
			t.Fatalf("column %s exists=%v err=%v", column, exists, err)
		}
	}
	var index string
	if err := db.QueryRow(`SELECT name FROM sqlite_master WHERE type='index' AND name='idx_member_managed_name'`).Scan(&index); err != nil {
		t.Fatal(err)
	}
}

func TestTaskReminderRoundTrip(t *testing.T) {
	s := openTestStore(t)
	ctx := context.Background()
	now := time.Now().UTC().Truncate(time.Second)
	due, reminded := now.Add(time.Hour), now.Add(2*time.Hour)
	task := groupmgr.Task{GroupID: 8, Title: "跟进事项", Status: groupmgr.TaskPending, DueAt: &due, RemindedAt: &reminded, CreatedAt: now, UpdatedAt: now}
	if err := s.UpsertTask(ctx, &task); err != nil {
		t.Fatal(err)
	}
	tasks, err := s.ListTasks(ctx, 8, groupmgr.TaskPending)
	if err != nil || len(tasks) != 1 || tasks[0].DueAt == nil || tasks[0].RemindedAt == nil {
		t.Fatalf("tasks=%#v err=%v", tasks, err)
	}
	if !tasks[0].DueAt.Equal(due) || !tasks[0].RemindedAt.Equal(reminded) {
		t.Fatalf("task times=%#v", tasks[0])
	}
}

func TestMessageIdempotencyAndRuleRoundTrip(t *testing.T) {
	s := openTestStore(t)
	ctx := context.Background()
	now := time.Now().UTC()
	message := groupmgr.Message{AccountID: "a", GroupID: 8, ServerMessageID: "m1", Sequence: 3, UserID: 9, Kind: groupmgr.MessageText, Text: "hello", SentAt: now, ReceivedAt: now}
	inserted, err := s.InsertMessage(ctx, &message)
	if err != nil || !inserted || message.ID == 0 {
		t.Fatalf("first insert=%v id=%d err=%v", inserted, message.ID, err)
	}
	duplicate := message
	duplicate.ID = 0
	inserted, err = s.InsertMessage(ctx, &duplicate)
	if err != nil || inserted {
		t.Fatalf("duplicate insert=%v err=%v", inserted, err)
	}
	var count int
	if err := s.DB.QueryRowContext(ctx, "SELECT count(*) FROM messages").Scan(&count); err != nil || count != 1 {
		t.Fatalf("count=%d err=%v", count, err)
	}
	rule := groupmgr.ModerationRule{GroupID: 8, Name: "ads", Matcher: groupmgr.MatcherContains, Pattern: "ad", Mode: groupmgr.RuleAuto, Enabled: true, Window: 10 * time.Minute, Cooldown: time.Minute, ExemptRoles: []groupmgr.MemberRole{groupmgr.RoleAdmin}, ExemptUserIDs: []int64{42}, Actions: []groupmgr.RuleAction{{Type: groupmgr.ActionMute, Duration: 10 * time.Minute}}, CreatedAt: now, UpdatedAt: now}
	if err := s.UpsertRule(ctx, &rule); err != nil {
		t.Fatal(err)
	}
	rules, err := s.ListRules(ctx, 8)
	if err != nil {
		t.Fatal(err)
	}
	if len(rules) != 1 || rules[0].Actions[0].Duration != 10*time.Minute || rules[0].ExemptUserIDs[0] != 42 {
		t.Fatalf("unexpected rule: %#v", rules)
	}
}

func TestCleanupRetention(t *testing.T) {
	s := openTestStore(t)
	ctx := context.Background()
	now := time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC)
	for id, received := range map[string]time.Time{"old": now.Add(-31 * 24 * time.Hour), "new": now.Add(-29 * 24 * time.Hour)} {
		m := groupmgr.Message{AccountID: "a", GroupID: 1, ServerMessageID: id, UserID: 1, Kind: groupmgr.MessageText, SentAt: received, ReceivedAt: received}
		if _, err := s.InsertMessage(ctx, &m); err != nil {
			t.Fatal(err)
		}
	}
	for event, created := range map[string]time.Time{"old": now.Add(-181 * 24 * time.Hour), "new": now.Add(-179 * 24 * time.Hour)} {
		if _, err := s.RecordAudit(ctx, groupmgr.AuditEvent{Event: event, CreatedAt: created}); err != nil {
			t.Fatal(err)
		}
	}
	if err := s.Cleanup(ctx, now); err != nil {
		t.Fatal(err)
	}
	assertCount(t, s.DB, "messages", 1)
	assertCount(t, s.DB, "audit_events", 1)
}

func assertCount(t *testing.T, db *sql.DB, table string, want int) {
	t.Helper()
	var got int
	if err := db.QueryRow("SELECT count(*) FROM " + table).Scan(&got); err != nil || got != want {
		t.Fatalf("%s count=%d want=%d err=%v", table, got, want, err)
	}
}

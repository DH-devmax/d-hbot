package store

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"dh/internal/groupmgr"
	_ "modernc.org/sqlite"
)

const schema = `
CREATE TABLE IF NOT EXISTS groups (
 account_id TEXT NOT NULL DEFAULT '', group_id INTEGER NOT NULL, name TEXT NOT NULL DEFAULT '', owner_user_id INTEGER NOT NULL DEFAULT 0,
 enabled INTEGER NOT NULL DEFAULT 0, ai_enabled INTEGER NOT NULL DEFAULT 0, moderation_enabled INTEGER NOT NULL DEFAULT 0, manual_takeover INTEGER NOT NULL DEFAULT 0, task_reminders_enabled INTEGER NOT NULL DEFAULT 1,
 welcome_message TEXT NOT NULL DEFAULT '', summary_schedule TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, updated_at TEXT NOT NULL, synced_at TEXT NOT NULL DEFAULT '', PRIMARY KEY(account_id, group_id)
);
CREATE TABLE IF NOT EXISTS members (
 account_id TEXT NOT NULL DEFAULT '', group_id INTEGER NOT NULL, user_id INTEGER NOT NULL, nim_id TEXT NOT NULL DEFAULT '', nickname TEXT NOT NULL DEFAULT '', card_name TEXT NOT NULL DEFAULT '', account_state TEXT NOT NULL DEFAULT '', original_card_name TEXT NOT NULL DEFAULT '', managed_card_name TEXT NOT NULL DEFAULT '', card_suffix TEXT NOT NULL DEFAULT '', role TEXT NOT NULL DEFAULT 'member',
 blacklisted INTEGER NOT NULL DEFAULT 0, present INTEGER NOT NULL DEFAULT 1, card_status TEXT NOT NULL DEFAULT 'unmanaged', join_source TEXT NOT NULL DEFAULT 'baseline', notice_read INTEGER NOT NULL DEFAULT 1, locked_card_name TEXT NOT NULL DEFAULT '', rename_violations INTEGER NOT NULL DEFAULT 0, joined_at TEXT NOT NULL DEFAULT '', discovered_at TEXT NOT NULL DEFAULT '', last_seen_at TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL,
 PRIMARY KEY(account_id, group_id, user_id), FOREIGN KEY(account_id, group_id) REFERENCES groups(account_id, group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS group_card_settings (
 account_id TEXT NOT NULL DEFAULT '', group_id INTEGER NOT NULL, prefix TEXT NOT NULL DEFAULT 'DH', auto_rename INTEGER NOT NULL DEFAULT 0, paused INTEGER NOT NULL DEFAULT 0,
 baseline_at TEXT NOT NULL DEFAULT '', last_snapshot_at TEXT NOT NULL DEFAULT '', reported_count INTEGER NOT NULL DEFAULT 0, resolved_count INTEGER NOT NULL DEFAULT 0, roster_complete INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL,
 PRIMARY KEY(account_id,group_id), FOREIGN KEY(account_id,group_id) REFERENCES groups(account_id,group_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS card_rename_jobs (
 id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT NOT NULL DEFAULT '', group_id INTEGER NOT NULL, user_id INTEGER NOT NULL, nim_id TEXT NOT NULL DEFAULT '', original_name TEXT NOT NULL DEFAULT '', desired_name TEXT NOT NULL, suffix TEXT NOT NULL DEFAULT '',
 source TEXT NOT NULL DEFAULT 'baseline', state TEXT NOT NULL DEFAULT 'queued', attempts INTEGER NOT NULL DEFAULT 0, next_attempt_at TEXT NOT NULL DEFAULT '', last_error TEXT NOT NULL DEFAULT '', welcome_pending INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 FOREIGN KEY(account_id,group_id,user_id) REFERENCES members(account_id,group_id,user_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS messages (
 id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT NOT NULL DEFAULT '', group_id INTEGER NOT NULL, server_message_id TEXT NOT NULL, sequence INTEGER NOT NULL DEFAULT 0, user_id INTEGER NOT NULL,
 sender_name TEXT NOT NULL DEFAULT '', kind TEXT NOT NULL DEFAULT 'text', text TEXT NOT NULL DEFAULT '', sent_at TEXT NOT NULL, received_at TEXT NOT NULL, processed_at TEXT NOT NULL DEFAULT '', acknowledged_at TEXT NOT NULL DEFAULT '',
 UNIQUE(account_id, group_id, server_message_id)
);
CREATE TABLE IF NOT EXISTS daily_summaries (
 id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT NOT NULL DEFAULT '', group_id INTEGER NOT NULL, content TEXT NOT NULL, source TEXT NOT NULL DEFAULT 'scheduled', created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS rules (
 id INTEGER PRIMARY KEY AUTOINCREMENT, group_id INTEGER NOT NULL, name TEXT NOT NULL, matcher TEXT NOT NULL, pattern TEXT NOT NULL DEFAULT '', threshold INTEGER NOT NULL DEFAULT 0, count INTEGER NOT NULL DEFAULT 0,
 window_ns INTEGER NOT NULL DEFAULT 0, cooldown_ns INTEGER NOT NULL DEFAULT 0, priority INTEGER NOT NULL DEFAULT 0, mode TEXT NOT NULL DEFAULT 'observe', enabled INTEGER NOT NULL DEFAULT 1, semantic_threshold REAL NOT NULL DEFAULT 0,
 exempt_roles TEXT NOT NULL DEFAULT '[]', exempt_user_ids TEXT NOT NULL DEFAULT '[]', actions TEXT NOT NULL DEFAULT '[]', created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS knowledge (id INTEGER PRIMARY KEY AUTOINCREMENT, group_id INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT '', title TEXT NOT NULL DEFAULT '', content TEXT NOT NULL DEFAULT '', source TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS knowledge_bases (id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT NOT NULL DEFAULT '', name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', enabled INTEGER NOT NULL DEFAULT 1, built_in INTEGER NOT NULL DEFAULT 0, read_only INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, UNIQUE(account_id,name));
CREATE TABLE IF NOT EXISTS knowledge_documents (id INTEGER PRIMARY KEY AUTOINCREMENT, base_id INTEGER NOT NULL, title TEXT NOT NULL, kind TEXT NOT NULL DEFAULT '', content TEXT NOT NULL, source TEXT NOT NULL DEFAULT '', content_hash TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, updated_at TEXT NOT NULL, FOREIGN KEY(base_id) REFERENCES knowledge_bases(id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS knowledge_base_groups (base_id INTEGER NOT NULL, account_id TEXT NOT NULL DEFAULT '', group_id INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL, PRIMARY KEY(base_id,account_id,group_id), FOREIGN KEY(base_id) REFERENCES knowledge_bases(id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS tasks (id INTEGER PRIMARY KEY AUTOINCREMENT, group_id INTEGER NOT NULL, assignee_id INTEGER NOT NULL DEFAULT 0, created_by_id INTEGER NOT NULL DEFAULT 0, title TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'pending', due_at TEXT NOT NULL DEFAULT '', reminded_at TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS actions (id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT NOT NULL DEFAULT '', group_id INTEGER NOT NULL, user_id INTEGER NOT NULL DEFAULT 0, message_id INTEGER NOT NULL DEFAULT 0, rule_id INTEGER NOT NULL DEFAULT 0, type TEXT NOT NULL, mode TEXT NOT NULL DEFAULT 'auto', duration_ns INTEGER NOT NULL DEFAULT 0, reason TEXT NOT NULL DEFAULT '', success INTEGER NOT NULL DEFAULT 0, error TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS audit_events (id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT NOT NULL DEFAULT '', group_id INTEGER NOT NULL DEFAULT 0, user_id INTEGER NOT NULL DEFAULT 0, actor TEXT NOT NULL DEFAULT '', event TEXT NOT NULL, level TEXT NOT NULL DEFAULT 'info', details TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL DEFAULT '', sensitive INTEGER NOT NULL DEFAULT 0, updated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS group_schedules (
 id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT NOT NULL DEFAULT '', name TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 0,
 open_time TEXT NOT NULL, close_time TEXT NOT NULL, timezone TEXT NOT NULL DEFAULT 'Local', created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
 UNIQUE(account_id,name)
);
CREATE TABLE IF NOT EXISTS group_schedule_groups (
 schedule_id INTEGER NOT NULL, account_id TEXT NOT NULL DEFAULT '', group_id INTEGER NOT NULL, enabled INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
 PRIMARY KEY(schedule_id,account_id,group_id), FOREIGN KEY(schedule_id) REFERENCES group_schedules(id) ON DELETE CASCADE,
 UNIQUE(account_id,group_id)
);
CREATE TABLE IF NOT EXISTS schedule_runs (
 id INTEGER PRIMARY KEY AUTOINCREMENT, schedule_id INTEGER NOT NULL, account_id TEXT NOT NULL DEFAULT '', group_id INTEGER NOT NULL,
 local_date TEXT NOT NULL, action TEXT NOT NULL, run_key TEXT NOT NULL UNIQUE, success INTEGER NOT NULL DEFAULT 0, error TEXT NOT NULL DEFAULT '', attempts INTEGER NOT NULL DEFAULT 0, next_retry_at TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL,
 FOREIGN KEY(schedule_id) REFERENCES group_schedules(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_messages_received ON messages(received_at);
CREATE INDEX IF NOT EXISTS idx_audit_created ON audit_events(created_at);
CREATE INDEX IF NOT EXISTS idx_actions_group ON actions(group_id, created_at);
CREATE INDEX IF NOT EXISTS idx_card_jobs_due ON card_rename_jobs(account_id,state,next_attempt_at,id);
CREATE INDEX IF NOT EXISTS idx_daily_summaries_created ON daily_summaries(account_id,created_at);
`

type Store struct {
	DB   *sql.DB
	Path string
}

const schemaVersion = 9

func DefaultPath() string {
	base, err := os.UserConfigDir()
	if err != nil || base == "" {
		base = "."
	}
	return filepath.Join(base, "DH", "dh.db")
}

func Open(path string) (*Store, error) {
	if path == "" {
		path = DefaultPath()
	}
	if dir := filepath.Dir(path); dir != "." {
		if err := os.MkdirAll(dir, 0o700); err != nil {
			return nil, err
		}
	}
	db, err := sql.Open("sqlite", path)
	if err != nil {
		return nil, fmt.Errorf("open sqlite: %w", err)
	}
	// SQLite PRAGMAs such as foreign_keys are connection-local. Keeping one
	// connection makes their behavior deterministic while group workers provide
	// the higher-level concurrency boundary.
	db.SetMaxOpenConns(1)
	s := &Store{DB: db, Path: path}
	if err := s.prepare(context.Background()); err != nil {
		_ = db.Close()
		return nil, err
	}
	if err := s.CheckIntegrity(context.Background()); err != nil {
		_ = db.Close()
		return nil, err
	}
	version, err := s.UserVersion(context.Background())
	if err != nil {
		_ = db.Close()
		return nil, err
	}
	if version > 0 && version < schemaVersion {
		if _, err := s.BackupBeforeMigration(context.Background(), 3); err != nil {
			_ = db.Close()
			return nil, err
		}
	}
	if err = s.Migrate(context.Background()); err != nil {
		_ = db.Close()
		return nil, err
	}
	return s, nil
}

func New(db *sql.DB) *Store { return &Store{DB: db} }
func (s *Store) Close() error {
	if s == nil || s.DB == nil {
		return nil
	}
	return s.DB.Close()
}

func (s *Store) Migrate(ctx context.Context) error {
	if s == nil || s.DB == nil {
		return errors.New("store: nil database")
	}
	if err := s.prepare(ctx); err != nil {
		return err
	}
	tx, err := s.DB.BeginTx(ctx, nil)
	if err != nil {
		return fmt.Errorf("begin migration: %w", err)
	}
	rollback := func(err error) error {
		_ = tx.Rollback()
		return err
	}
	if _, err := tx.ExecContext(ctx, schema); err != nil {
		return rollback(fmt.Errorf("migrate: %w", err))
	}
	for _, migration := range []struct{ table, column, definition string }{
		{"groups", "task_reminders_enabled", "task_reminders_enabled INTEGER NOT NULL DEFAULT 1"},
		{"tasks", "reminded_at", "reminded_at TEXT NOT NULL DEFAULT ''"},
		{"members", "nim_id", "nim_id TEXT NOT NULL DEFAULT ''"},
		{"members", "account_state", "account_state TEXT NOT NULL DEFAULT ''"},
		{"members", "original_card_name", "original_card_name TEXT NOT NULL DEFAULT ''"},
		{"members", "managed_card_name", "managed_card_name TEXT NOT NULL DEFAULT ''"},
		{"members", "card_suffix", "card_suffix TEXT NOT NULL DEFAULT ''"},
		{"members", "present", "present INTEGER NOT NULL DEFAULT 1"},
		{"members", "card_status", "card_status TEXT NOT NULL DEFAULT 'unmanaged'"},
		{"members", "join_source", "join_source TEXT NOT NULL DEFAULT 'baseline'"},
		{"members", "notice_read", "notice_read INTEGER NOT NULL DEFAULT 1"},
		{"members", "discovered_at", "discovered_at TEXT NOT NULL DEFAULT ''"},
		{"schedule_runs", "attempts", "attempts INTEGER NOT NULL DEFAULT 0"},
		{"schedule_runs", "next_retry_at", "next_retry_at TEXT NOT NULL DEFAULT ''"},
	} {
		exists, err := hasColumn(ctx, tx, migration.table, migration.column)
		if err != nil {
			return rollback(err)
		}
		if !exists {
			if _, err := tx.ExecContext(ctx, "ALTER TABLE "+migration.table+" ADD COLUMN "+migration.definition); err != nil {
				return rollback(fmt.Errorf("migrate %s.%s: %w", migration.table, migration.column, err))
			}
		}
	}
	if _, err := tx.ExecContext(ctx, `CREATE UNIQUE INDEX IF NOT EXISTS idx_member_managed_name ON members(account_id,group_id,managed_card_name) WHERE managed_card_name<>''`); err != nil {
		return rollback(fmt.Errorf("migrate managed member index: %w", err))
	}
	if _, err := tx.ExecContext(ctx, "PRAGMA user_version = 9"); err != nil {
		return rollback(fmt.Errorf("set schema version: %w", err))
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("commit migration: %w", err)
	}
	return nil
}

func (s *Store) hasColumn(ctx context.Context, table, column string) (bool, error) {
	return hasColumn(ctx, s.DB, table, column)
}

type queryer interface {
	QueryContext(context.Context, string, ...any) (*sql.Rows, error)
}

func hasColumn(ctx context.Context, q queryer, table, column string) (bool, error) {
	rows, err := q.QueryContext(ctx, "PRAGMA table_info("+table+")")
	if err != nil {
		return false, err
	}
	defer rows.Close()
	for rows.Next() {
		var cid, notNull, primaryKey int
		var name, dataType string
		var defaultValue sql.NullString
		if err := rows.Scan(&cid, &name, &dataType, &notNull, &defaultValue, &primaryKey); err != nil {
			return false, err
		}
		if name == column {
			return true, nil
		}
	}
	return false, rows.Err()
}

func (s *Store) prepare(ctx context.Context) error {
	for _, q := range []string{"PRAGMA foreign_keys = ON", "PRAGMA journal_mode = WAL", "PRAGMA busy_timeout = 5000"} {
		if _, err := s.DB.ExecContext(ctx, q); err != nil {
			return fmt.Errorf("sqlite %s: %w", q, err)
		}
	}
	return nil
}

// CheckIntegrity runs before migrations so a damaged database is preserved for
// recovery instead of being modified during startup.
func (s *Store) CheckIntegrity(ctx context.Context) error {
	if s == nil || s.DB == nil {
		return errors.New("store: nil database")
	}
	var result string
	if err := s.DB.QueryRowContext(ctx, "PRAGMA quick_check").Scan(&result); err != nil {
		return fmt.Errorf("sqlite integrity check: %w", err)
	}
	if !strings.EqualFold(strings.TrimSpace(result), "ok") {
		return fmt.Errorf("sqlite integrity check: %s", result)
	}
	return nil
}

func (s *Store) UserVersion(ctx context.Context) (int, error) {
	if s == nil || s.DB == nil {
		return 0, errors.New("store: nil database")
	}
	var version int
	if err := s.DB.QueryRowContext(ctx, "PRAGMA user_version").Scan(&version); err != nil {
		return 0, fmt.Errorf("read sqlite schema version: %w", err)
	}
	return version, nil
}

// BackupBeforeMigration creates a consistent SQLite snapshot and keeps the
// newest maxBackups files in %APPDATA%\\DH\\backups.
func (s *Store) BackupBeforeMigration(ctx context.Context, maxBackups int) (string, error) {
	if s == nil || s.DB == nil || s.Path == "" || strings.HasPrefix(s.Path, ":") {
		return "", nil
	}
	if maxBackups < 1 {
		maxBackups = 1
	}
	dir := filepath.Join(filepath.Dir(s.Path), "backups")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return "", fmt.Errorf("create sqlite backup directory: %w", err)
	}
	name := fmt.Sprintf("%s-%s.db", strings.TrimSuffix(filepath.Base(s.Path), filepath.Ext(s.Path)), time.Now().UTC().Format("20060102-150405.000000000"))
	destination := filepath.Join(dir, name)
	escaped := strings.ReplaceAll(destination, "'", "''")
	if _, err := s.DB.ExecContext(ctx, "VACUUM INTO '"+escaped+"'"); err != nil {
		return "", fmt.Errorf("backup sqlite database: %w", err)
	}
	entries, err := os.ReadDir(dir)
	if err != nil {
		return destination, nil
	}
	files := make([]string, 0, len(entries))
	for _, entry := range entries {
		if !entry.IsDir() && strings.HasSuffix(entry.Name(), ".db") {
			files = append(files, filepath.Join(dir, entry.Name()))
		}
	}
	sort.Strings(files)
	for len(files) > maxBackups {
		_ = os.Remove(files[0])
		files = files[1:]
	}
	return destination, nil
}

func nowText(t time.Time) string {
	if t.IsZero() {
		t = time.Now().UTC()
	}
	return t.UTC().Format(time.RFC3339Nano)
}
func parseTime(v sql.NullString) time.Time {
	if !v.Valid || v.String == "" {
		return time.Time{}
	}
	t, _ := time.Parse(time.RFC3339Nano, v.String)
	return t
}
func boolInt(v bool) int {
	if v {
		return 1
	}
	return 0
}
func intBool(v int64) bool { return v != 0 }

func (s *Store) UpsertGroup(ctx context.Context, g groupmgr.Group) error {
	_, err := s.DB.ExecContext(ctx, `INSERT INTO groups(account_id,group_id,name,owner_user_id,enabled,ai_enabled,moderation_enabled,manual_takeover,task_reminders_enabled,welcome_message,summary_schedule,created_at,updated_at,synced_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,group_id) DO UPDATE SET name=excluded.name,owner_user_id=excluded.owner_user_id,enabled=excluded.enabled,ai_enabled=excluded.ai_enabled,moderation_enabled=excluded.moderation_enabled,manual_takeover=excluded.manual_takeover,task_reminders_enabled=excluded.task_reminders_enabled,welcome_message=excluded.welcome_message,summary_schedule=excluded.summary_schedule,updated_at=excluded.updated_at,synced_at=excluded.synced_at`, g.AccountID, g.GroupID, g.Name, g.OwnerUserID, boolInt(g.Enabled), boolInt(g.AIEnabled), boolInt(g.ModerationEnabled), boolInt(g.ManualTakeover), boolInt(g.TaskReminders), g.WelcomeMessage, g.SummarySchedule, nowText(g.CreatedAt), nowText(g.UpdatedAt), optionalTimeText(g.SyncedAt))
	return err
}
func (s *Store) CreateGroup(ctx context.Context, g groupmgr.Group) error {
	return s.UpsertGroup(ctx, g)
}

func (s *Store) ListGroups(ctx context.Context, accountID string, enabledOnly bool) ([]groupmgr.Group, error) {
	q := `SELECT account_id,group_id,name,owner_user_id,enabled,ai_enabled,moderation_enabled,manual_takeover,task_reminders_enabled,welcome_message,summary_schedule,created_at,updated_at,synced_at FROM groups WHERE account_id=?`
	args := []any{accountID}
	if enabledOnly {
		q += " AND enabled=1"
	}
	q += " ORDER BY name,group_id"
	rows, err := s.DB.QueryContext(ctx, q, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []groupmgr.Group
	for rows.Next() {
		var g groupmgr.Group
		var en, ai, mod, take, reminders int64
		var c, u, sy sql.NullString
		if err := rows.Scan(&g.AccountID, &g.GroupID, &g.Name, &g.OwnerUserID, &en, &ai, &mod, &take, &reminders, &g.WelcomeMessage, &g.SummarySchedule, &c, &u, &sy); err != nil {
			return nil, err
		}
		g.Enabled = intBool(en)
		g.AIEnabled = intBool(ai)
		g.ModerationEnabled = intBool(mod)
		g.ManualTakeover = intBool(take)
		g.TaskReminders = intBool(reminders)
		g.CreatedAt = parseTime(c)
		g.UpdatedAt = parseTime(u)
		g.SyncedAt = parseTime(sy)
		out = append(out, g)
	}
	return out, rows.Err()
}

func (s *Store) UpsertMember(ctx context.Context, m groupmgr.Member) error {
	if m.OriginalCardName == "" {
		m.OriginalCardName = m.CardName
	}
	if m.CardStatus == "" {
		m.CardStatus = groupmgr.CardUnmanaged
	}
	if m.JoinSource == "" {
		m.JoinSource = groupmgr.JoinBaseline
		m.NoticeRead = true
		m.Present = true
	}
	if m.DiscoveredAt.IsZero() {
		m.DiscoveredAt = time.Now().UTC()
	}
	_, err := s.DB.ExecContext(ctx, `INSERT INTO members(account_id,group_id,user_id,nim_id,nickname,card_name,account_state,original_card_name,managed_card_name,card_suffix,role,blacklisted,present,card_status,join_source,notice_read,locked_card_name,rename_violations,joined_at,discovered_at,last_seen_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,group_id,user_id) DO UPDATE SET nim_id=excluded.nim_id,nickname=excluded.nickname,card_name=excluded.card_name,account_state=excluded.account_state,original_card_name=excluded.original_card_name,managed_card_name=excluded.managed_card_name,card_suffix=excluded.card_suffix,role=excluded.role,blacklisted=excluded.blacklisted,present=excluded.present,card_status=excluded.card_status,join_source=excluded.join_source,notice_read=excluded.notice_read,locked_card_name=excluded.locked_card_name,rename_violations=excluded.rename_violations,discovered_at=excluded.discovered_at,last_seen_at=excluded.last_seen_at,updated_at=excluded.updated_at`, m.AccountID, m.GroupID, m.UserID, m.NIMID, m.Nickname, m.CardName, m.AccountState, m.OriginalCardName, m.ManagedCardName, m.CardSuffix, m.Role, boolInt(m.Blacklisted), boolInt(m.Present), m.CardStatus, m.JoinSource, boolInt(m.NoticeRead), m.LockedCardName, m.RenameViolations, optionalTimeText(m.JoinedAt), optionalTimeText(m.DiscoveredAt), optionalTimeText(m.LastSeenAt), nowText(m.UpdatedAt))
	return err
}
func (s *Store) ListMembers(ctx context.Context, accountID string, groupID int64) ([]groupmgr.Member, error) {
	return s.listMembers(ctx, accountID, groupID, true)
}

func (s *Store) ListAllMembers(ctx context.Context, accountID string, groupID int64) ([]groupmgr.Member, error) {
	return s.listMembers(ctx, accountID, groupID, false)
}

func (s *Store) listMembers(ctx context.Context, accountID string, groupID int64, presentOnly bool) ([]groupmgr.Member, error) {
	query := memberSelect + ` WHERE account_id=? AND group_id=?`
	if presentOnly {
		query += ` AND present=1`
	}
	query += ` ORDER BY nickname,user_id`
	rows, err := s.DB.QueryContext(ctx, query, accountID, groupID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []groupmgr.Member
	for rows.Next() {
		m, err := scanMember(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, m)
	}
	return out, rows.Err()
}

func (s *Store) DeleteMembersExcept(ctx context.Context, accountID string, groupID int64, keep []int64) error {
	if len(keep) == 0 {
		_, err := s.DB.ExecContext(ctx, `UPDATE members SET present=0,updated_at=? WHERE account_id=? AND group_id=?`, nowText(time.Now()), accountID, groupID)
		return err
	}
	placeholders := strings.TrimRight(strings.Repeat("?,", len(keep)), ",")
	arguments := make([]any, 0, len(keep)+2)
	arguments = append(arguments, accountID, groupID)
	for _, userID := range keep {
		arguments = append(arguments, userID)
	}
	arguments = append([]any{nowText(time.Now())}, arguments...)
	_, err := s.DB.ExecContext(ctx, `UPDATE members SET present=0,updated_at=? WHERE account_id=? AND group_id=? AND user_id NOT IN (`+placeholders+`)`, arguments...)
	return err
}

func (s *Store) InsertMessage(ctx context.Context, m *groupmgr.Message) (bool, error) {
	r, err := s.DB.ExecContext(ctx, `INSERT INTO messages(account_id,group_id,server_message_id,sequence,user_id,sender_name,kind,text,sent_at,received_at,processed_at,acknowledged_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,group_id,server_message_id) DO NOTHING`, m.AccountID, m.GroupID, m.ServerMessageID, m.Sequence, m.UserID, m.SenderName, m.Kind, m.Text, nowText(m.SentAt), nowText(m.ReceivedAt), timeText(m.ProcessedAt), timeText(m.AcknowledgedAt))
	if err != nil {
		return false, err
	}
	n, _ := r.RowsAffected()
	if n == 0 {
		return false, nil
	}
	id, _ := r.LastInsertId()
	m.ID = id
	return true, nil
}
func timeText(t *time.Time) string {
	if t == nil || t.IsZero() {
		return ""
	}
	return nowText(*t)
}
func optionalTimeText(t time.Time) string {
	if t.IsZero() {
		return ""
	}
	return nowText(t)
}
func (s *Store) MarkMessageProcessed(ctx context.Context, id int64, ack bool) error {
	now := nowText(time.Now())
	if ack {
		return s.markMessage(ctx, id, "acknowledged_at", now)
	}
	return s.markMessage(ctx, id, "processed_at", now)
}

func (s *Store) AcknowledgeMessage(ctx context.Context, id int64) error {
	return s.markMessage(ctx, id, "acknowledged_at", nowText(time.Now()))
}

func (s *Store) SetMessageProcessed(ctx context.Context, id int64) error {
	return s.markMessage(ctx, id, "processed_at", nowText(time.Now()))
}

func (s *Store) UpsertRule(ctx context.Context, rule *groupmgr.ModerationRule) error {
	roles, err := json.Marshal(rule.ExemptRoles)
	if err != nil {
		return err
	}
	users, err := json.Marshal(rule.ExemptUserIDs)
	if err != nil {
		return err
	}
	actions, err := json.Marshal(rule.Actions)
	if err != nil {
		return err
	}
	created, updated := nowText(rule.CreatedAt), nowText(rule.UpdatedAt)
	if rule.ID == 0 {
		result, err := s.DB.ExecContext(ctx, `INSERT INTO rules(group_id,name,matcher,pattern,threshold,count,window_ns,cooldown_ns,priority,mode,enabled,semantic_threshold,exempt_roles,exempt_user_ids,actions,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)`, rule.GroupID, rule.Name, rule.Matcher, rule.Pattern, rule.Threshold, rule.Count, rule.Window.Nanoseconds(), rule.Cooldown.Nanoseconds(), rule.Priority, rule.Mode, boolInt(rule.Enabled), rule.SemanticThreshold, string(roles), string(users), string(actions), created, updated)
		if err != nil {
			return err
		}
		rule.ID, err = result.LastInsertId()
		return err
	}
	_, err = s.DB.ExecContext(ctx, `INSERT INTO rules(id,group_id,name,matcher,pattern,threshold,count,window_ns,cooldown_ns,priority,mode,enabled,semantic_threshold,exempt_roles,exempt_user_ids,actions,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET group_id=excluded.group_id,name=excluded.name,matcher=excluded.matcher,pattern=excluded.pattern,threshold=excluded.threshold,count=excluded.count,window_ns=excluded.window_ns,cooldown_ns=excluded.cooldown_ns,priority=excluded.priority,mode=excluded.mode,enabled=excluded.enabled,semantic_threshold=excluded.semantic_threshold,exempt_roles=excluded.exempt_roles,exempt_user_ids=excluded.exempt_user_ids,actions=excluded.actions,updated_at=excluded.updated_at`, rule.ID, rule.GroupID, rule.Name, rule.Matcher, rule.Pattern, rule.Threshold, rule.Count, rule.Window.Nanoseconds(), rule.Cooldown.Nanoseconds(), rule.Priority, rule.Mode, boolInt(rule.Enabled), rule.SemanticThreshold, string(roles), string(users), string(actions), created, updated)
	return err
}

func (s *Store) ListRules(ctx context.Context, groupID int64) ([]groupmgr.ModerationRule, error) {
	rows, err := s.DB.QueryContext(ctx, `SELECT id,group_id,name,matcher,pattern,threshold,count,window_ns,cooldown_ns,priority,mode,enabled,semantic_threshold,exempt_roles,exempt_user_ids,actions,created_at,updated_at FROM rules WHERE group_id IN (0,?) ORDER BY priority DESC,id`, groupID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []groupmgr.ModerationRule
	for rows.Next() {
		var r groupmgr.ModerationRule
		var window, cooldown, enabled int64
		var roles, users, actions string
		var created, updated sql.NullString
		if err := rows.Scan(&r.ID, &r.GroupID, &r.Name, &r.Matcher, &r.Pattern, &r.Threshold, &r.Count, &window, &cooldown, &r.Priority, &r.Mode, &enabled, &r.SemanticThreshold, &roles, &users, &actions, &created, &updated); err != nil {
			return nil, err
		}
		r.Window = time.Duration(window)
		r.Cooldown = time.Duration(cooldown)
		r.Enabled = intBool(enabled)
		r.CreatedAt = parseTime(created)
		r.UpdatedAt = parseTime(updated)
		if err := json.Unmarshal([]byte(roles), &r.ExemptRoles); err != nil {
			return nil, fmt.Errorf("decode rule %d roles: %w", r.ID, err)
		}
		if err := json.Unmarshal([]byte(users), &r.ExemptUserIDs); err != nil {
			return nil, fmt.Errorf("decode rule %d users: %w", r.ID, err)
		}
		if err := json.Unmarshal([]byte(actions), &r.Actions); err != nil {
			return nil, fmt.Errorf("decode rule %d actions: %w", r.ID, err)
		}
		out = append(out, r)
	}
	return out, rows.Err()
}
func (s *Store) DeleteRule(ctx context.Context, id int64) error {
	_, err := s.DB.ExecContext(ctx, `DELETE FROM rules WHERE id=?`, id)
	return err
}

func (s *Store) DeleteNonGlobalRulesByNames(ctx context.Context, names []string) error {
	if len(names) == 0 {
		return nil
	}
	placeholders := strings.TrimSuffix(strings.Repeat("?,", len(names)), ",")
	args := make([]any, len(names))
	for index, name := range names {
		args[index] = name
	}
	_, err := s.DB.ExecContext(ctx, `DELETE FROM rules WHERE group_id<>0 AND name IN (`+placeholders+`)`, args...)
	return err
}

func (s *Store) UpsertKnowledge(ctx context.Context, item *groupmgr.Knowledge) error {
	created, updated := nowText(item.CreatedAt), nowText(item.UpdatedAt)
	if item.ID == 0 {
		r, err := s.DB.ExecContext(ctx, `INSERT INTO knowledge(group_id,kind,title,content,source,created_at,updated_at) VALUES(?,?,?,?,?,?,?)`, item.GroupID, item.Kind, item.Title, item.Content, item.Source, created, updated)
		if err != nil {
			return err
		}
		item.ID, err = r.LastInsertId()
		return err
	}
	_, err := s.DB.ExecContext(ctx, `INSERT INTO knowledge(id,group_id,kind,title,content,source,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET group_id=excluded.group_id,kind=excluded.kind,title=excluded.title,content=excluded.content,source=excluded.source,updated_at=excluded.updated_at`, item.ID, item.GroupID, item.Kind, item.Title, item.Content, item.Source, created, updated)
	return err
}
func (s *Store) ListKnowledge(ctx context.Context, groupID int64) ([]groupmgr.Knowledge, error) {
	rows, err := s.DB.QueryContext(ctx, `SELECT id,group_id,kind,title,content,source,created_at,updated_at FROM knowledge WHERE group_id IN (0,?) ORDER BY title,id`, groupID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []groupmgr.Knowledge
	for rows.Next() {
		var k groupmgr.Knowledge
		var c, u sql.NullString
		if err := rows.Scan(&k.ID, &k.GroupID, &k.Kind, &k.Title, &k.Content, &k.Source, &c, &u); err != nil {
			return nil, err
		}
		k.CreatedAt = parseTime(c)
		k.UpdatedAt = parseTime(u)
		out = append(out, k)
	}
	return out, rows.Err()
}
func (s *Store) DeleteKnowledge(ctx context.Context, id int64) error {
	_, err := s.DB.ExecContext(ctx, `DELETE FROM knowledge WHERE id=?`, id)
	return err
}

func (s *Store) UpsertTask(ctx context.Context, item *groupmgr.Task) error {
	created, updated := nowText(item.CreatedAt), nowText(item.UpdatedAt)
	if item.Status == "" {
		item.Status = groupmgr.TaskPending
	}
	if item.ID == 0 {
		r, err := s.DB.ExecContext(ctx, `INSERT INTO tasks(group_id,assignee_id,created_by_id,title,description,status,due_at,reminded_at,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?)`, item.GroupID, item.AssigneeID, item.CreatedByID, item.Title, item.Description, item.Status, timeText(item.DueAt), timeText(item.RemindedAt), created, updated)
		if err != nil {
			return err
		}
		item.ID, err = r.LastInsertId()
		return err
	}
	_, err := s.DB.ExecContext(ctx, `INSERT INTO tasks(id,group_id,assignee_id,created_by_id,title,description,status,due_at,reminded_at,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET group_id=excluded.group_id,assignee_id=excluded.assignee_id,title=excluded.title,description=excluded.description,status=excluded.status,due_at=excluded.due_at,reminded_at=excluded.reminded_at,updated_at=excluded.updated_at`, item.ID, item.GroupID, item.AssigneeID, item.CreatedByID, item.Title, item.Description, item.Status, timeText(item.DueAt), timeText(item.RemindedAt), created, updated)
	return err
}
func (s *Store) ListTasks(ctx context.Context, groupID int64, status groupmgr.TaskStatus) ([]groupmgr.Task, error) {
	q := `SELECT id,group_id,assignee_id,created_by_id,title,description,status,due_at,reminded_at,created_at,updated_at FROM tasks WHERE group_id=?`
	args := []any{groupID}
	if status != "" {
		q += " AND status=?"
		args = append(args, status)
	}
	q += " ORDER BY due_at,id"
	rows, err := s.DB.QueryContext(ctx, q, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []groupmgr.Task
	for rows.Next() {
		var item groupmgr.Task
		var due, reminded, c, u sql.NullString
		if err := rows.Scan(&item.ID, &item.GroupID, &item.AssigneeID, &item.CreatedByID, &item.Title, &item.Description, &item.Status, &due, &reminded, &c, &u); err != nil {
			return nil, err
		}
		d := parseTime(due)
		if !d.IsZero() {
			item.DueAt = &d
		}
		if value := parseTime(reminded); !value.IsZero() {
			item.RemindedAt = &value
		}
		item.CreatedAt = parseTime(c)
		item.UpdatedAt = parseTime(u)
		out = append(out, item)
	}
	return out, rows.Err()
}
func (s *Store) markMessage(ctx context.Context, id int64, col, v string) error {
	_, err := s.DB.ExecContext(ctx, "UPDATE messages SET "+col+"=? WHERE id=?", v, id)
	return err
}

func (s *Store) RecordAction(ctx context.Context, a groupmgr.ActionRecord) (int64, error) {
	r, err := s.DB.ExecContext(ctx, `INSERT INTO actions(account_id,group_id,user_id,message_id,rule_id,type,mode,duration_ns,reason,success,error,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)`, a.AccountID, a.GroupID, a.UserID, a.MessageID, a.RuleID, a.Type, a.Mode, a.Duration.Nanoseconds(), a.Reason, boolInt(a.Success), a.Error, nowText(a.CreatedAt))
	if err != nil {
		return 0, err
	}
	return r.LastInsertId()
}
func (s *Store) RecordAudit(ctx context.Context, a groupmgr.AuditEvent) (int64, error) {
	r, err := s.DB.ExecContext(ctx, `INSERT INTO audit_events(account_id,group_id,user_id,actor,event,level,details,created_at) VALUES(?,?,?,?,?,?,?,?)`, a.AccountID, a.GroupID, a.UserID, a.Actor, a.Event, a.Level, a.Details, nowText(a.CreatedAt))
	if err != nil {
		return 0, err
	}
	return r.LastInsertId()
}

func (s *Store) SaveDailySummary(ctx context.Context, summary *groupmgr.DailySummary) error {
	result, err := s.DB.ExecContext(ctx, `INSERT INTO daily_summaries(account_id,group_id,content,source,created_at) VALUES(?,?,?,?,?)`, summary.AccountID, summary.GroupID, summary.Content, summary.Source, nowText(summary.CreatedAt))
	if err == nil {
		summary.ID, _ = result.LastInsertId()
	}
	return err
}

func (s *Store) SetSetting(ctx context.Context, v groupmgr.AppSetting) error {
	_, err := s.DB.ExecContext(ctx, `INSERT INTO app_settings(key,value,sensitive,updated_at) VALUES(?,?,?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value,sensitive=excluded.sensitive,updated_at=excluded.updated_at`, v.Key, v.Value, boolInt(v.Sensitive), nowText(v.UpdatedAt))
	return err
}
func (s *Store) GetSetting(ctx context.Context, key string) (groupmgr.AppSetting, error) {
	var v groupmgr.AppSetting
	var b int64
	var t sql.NullString
	err := s.DB.QueryRowContext(ctx, `SELECT key,value,sensitive,updated_at FROM app_settings WHERE key=?`, key).Scan(&v.Key, &v.Value, &b, &t)
	v.Sensitive = intBool(b)
	v.UpdatedAt = parseTime(t)
	return v, err
}

func (s *Store) Cleanup(ctx context.Context, now time.Time) error {
	if now.IsZero() {
		now = time.Now().UTC()
	}
	if _, err := s.DB.ExecContext(ctx, `DELETE FROM messages WHERE received_at < ?`, nowText(now.Add(-30*24*time.Hour))); err != nil {
		return err
	}
	if _, err := s.DB.ExecContext(ctx, `DELETE FROM daily_summaries WHERE created_at < ?`, nowText(now.Add(-180*24*time.Hour))); err != nil {
		return err
	}
	_, err := s.DB.ExecContext(ctx, `DELETE FROM audit_events WHERE created_at < ?`, nowText(now.Add(-180*24*time.Hour)))
	return err
}

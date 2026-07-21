package store

import (
	"context"
	"database/sql"
	"time"

	"dh/internal/groupmgr"
)

func (s *Store) GetGroup(ctx context.Context, accountID string, groupID int64) (groupmgr.Group, error) {
	var g groupmgr.Group
	var enabled, ai, moderation, takeover, reminders int64
	var created, updated, synced sql.NullString
	err := s.DB.QueryRowContext(ctx, `SELECT account_id,group_id,name,owner_user_id,enabled,ai_enabled,moderation_enabled,manual_takeover,task_reminders_enabled,welcome_message,summary_schedule,created_at,updated_at,synced_at FROM groups WHERE account_id=? AND group_id=?`, accountID, groupID).Scan(&g.AccountID, &g.GroupID, &g.Name, &g.OwnerUserID, &enabled, &ai, &moderation, &takeover, &reminders, &g.WelcomeMessage, &g.SummarySchedule, &created, &updated, &synced)
	g.Enabled = intBool(enabled)
	g.AIEnabled = intBool(ai)
	g.ModerationEnabled = intBool(moderation)
	g.ManualTakeover = intBool(takeover)
	g.TaskReminders = intBool(reminders)
	g.CreatedAt = parseTime(created)
	g.UpdatedAt = parseTime(updated)
	g.SyncedAt = parseTime(synced)
	return g, err
}

func (s *Store) SetGroupEnabled(ctx context.Context, accountID string, groupID int64, enabled bool) error {
	_, err := s.DB.ExecContext(ctx, `UPDATE groups SET enabled=?,updated_at=? WHERE account_id=? AND group_id=?`, boolInt(enabled), nowText(time.Now()), accountID, groupID)
	return err
}

func (s *Store) GetMember(ctx context.Context, accountID string, groupID, userID int64) (groupmgr.Member, error) {
	return scanMember(s.DB.QueryRowContext(ctx, memberSelect+` WHERE account_id=? AND group_id=? AND user_id=?`, accountID, groupID, userID))
}

const memberSelect = `SELECT account_id,group_id,user_id,nim_id,nickname,card_name,account_state,original_card_name,managed_card_name,card_suffix,role,blacklisted,present,card_status,join_source,notice_read,locked_card_name,rename_violations,joined_at,discovered_at,last_seen_at,updated_at FROM members`

func scanMember(row scanner) (groupmgr.Member, error) {
	var member groupmgr.Member
	var blacklisted, present, noticeRead int64
	var joined, discovered, lastSeen, updated sql.NullString
	err := row.Scan(&member.AccountID, &member.GroupID, &member.UserID, &member.NIMID, &member.Nickname, &member.CardName, &member.AccountState, &member.OriginalCardName, &member.ManagedCardName, &member.CardSuffix, &member.Role, &blacklisted, &present, &member.CardStatus, &member.JoinSource, &noticeRead, &member.LockedCardName, &member.RenameViolations, &joined, &discovered, &lastSeen, &updated)
	member.Blacklisted = intBool(blacklisted)
	member.Present = intBool(present)
	member.NoticeRead = intBool(noticeRead)
	member.JoinedAt = parseTime(joined)
	member.DiscoveredAt = parseTime(discovered)
	member.LastSeenAt = parseTime(lastSeen)
	member.UpdatedAt = parseTime(updated)
	return member, err
}

func (s *Store) IsBlacklisted(ctx context.Context, accountID string, groupID, userID int64) (bool, error) {
	var count int
	err := s.DB.QueryRowContext(ctx, `SELECT COUNT(*) FROM (
SELECT 1 FROM members WHERE account_id=? AND group_id=? AND user_id=? AND blacklisted=1
UNION ALL
SELECT 1 FROM actions WHERE account_id=? AND group_id=? AND user_id=? AND type='blacklist' AND success=1
)`, accountID, groupID, userID, accountID, groupID, userID).Scan(&count)
	return count > 0, err
}

func (s *Store) ListPendingMessages(ctx context.Context, accountID string, limit int) ([]groupmgr.Message, error) {
	if limit <= 0 {
		limit = 100
	}
	rows, err := s.DB.QueryContext(ctx, `SELECT id,account_id,group_id,server_message_id,sequence,user_id,sender_name,kind,text,sent_at,received_at,processed_at,acknowledged_at FROM messages WHERE account_id=? AND processed_at='' ORDER BY sequence,id LIMIT ?`, accountID, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []groupmgr.Message
	for rows.Next() {
		m, err := scanMessage(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, m)
	}
	return out, rows.Err()
}

type scanner interface{ Scan(...any) error }

func scanMessage(row scanner) (groupmgr.Message, error) {
	var m groupmgr.Message
	var sent, received, processed, ack sql.NullString
	err := row.Scan(&m.ID, &m.AccountID, &m.GroupID, &m.ServerMessageID, &m.Sequence, &m.UserID, &m.SenderName, &m.Kind, &m.Text, &sent, &received, &processed, &ack)
	m.SentAt = parseTime(sent)
	m.ReceivedAt = parseTime(received)
	if t := parseTime(processed); !t.IsZero() {
		m.ProcessedAt = &t
	}
	if t := parseTime(ack); !t.IsZero() {
		m.AcknowledgedAt = &t
	}
	return m, err
}

func (s *Store) GetMessage(ctx context.Context, accountID string, groupID int64, serverMessageID string) (groupmgr.Message, error) {
	row := s.DB.QueryRowContext(ctx, `SELECT id,account_id,group_id,server_message_id,sequence,user_id,sender_name,kind,text,sent_at,received_at,processed_at,acknowledged_at FROM messages WHERE account_id=? AND group_id=? AND server_message_id=?`, accountID, groupID, serverMessageID)
	return scanMessage(row)
}

func (s *Store) ListRecentMessages(ctx context.Context, accountID string, groupID int64, limit int) ([]groupmgr.Message, error) {
	if limit <= 0 {
		limit = 100
	}
	rows, err := s.DB.QueryContext(ctx, `SELECT id,account_id,group_id,server_message_id,sequence,user_id,sender_name,kind,text,sent_at,received_at,processed_at,acknowledged_at FROM messages WHERE account_id=? AND (?=0 OR group_id=?) ORDER BY sent_at DESC,id DESC LIMIT ?`, accountID, groupID, groupID, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []groupmgr.Message
	for rows.Next() {
		m, err := scanMessage(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, m)
	}
	return out, rows.Err()
}

func (s *Store) ListActions(ctx context.Context, groupID int64, limit int) ([]groupmgr.ActionRecord, error) {
	if limit <= 0 {
		limit = 100
	}
	rows, err := s.DB.QueryContext(ctx, `SELECT id,account_id,group_id,user_id,message_id,rule_id,type,mode,duration_ns,reason,success,error,created_at FROM actions WHERE (?=0 OR group_id=?) ORDER BY id DESC LIMIT ?`, groupID, groupID, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []groupmgr.ActionRecord
	for rows.Next() {
		var a groupmgr.ActionRecord
		var duration, success int64
		var created sql.NullString
		if err := rows.Scan(&a.ID, &a.AccountID, &a.GroupID, &a.UserID, &a.MessageID, &a.RuleID, &a.Type, &a.Mode, &duration, &a.Reason, &success, &a.Error, &created); err != nil {
			return nil, err
		}
		a.Duration = time.Duration(duration)
		a.Success = intBool(success)
		a.CreatedAt = parseTime(created)
		out = append(out, a)
	}
	return out, rows.Err()
}

func (s *Store) ListRecentActions(ctx context.Context, groupID int64, limit int) ([]groupmgr.ActionRecord, error) {
	return s.ListActions(ctx, groupID, limit)
}

func (s *Store) ListAudit(ctx context.Context, groupID int64, limit int) ([]groupmgr.AuditEvent, error) {
	if limit <= 0 {
		limit = 100
	}
	rows, err := s.DB.QueryContext(ctx, `SELECT id,account_id,group_id,user_id,actor,event,level,details,created_at FROM audit_events WHERE (?=0 OR group_id=?) ORDER BY id DESC LIMIT ?`, groupID, groupID, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []groupmgr.AuditEvent
	for rows.Next() {
		var a groupmgr.AuditEvent
		var created sql.NullString
		if err := rows.Scan(&a.ID, &a.AccountID, &a.GroupID, &a.UserID, &a.Actor, &a.Event, &a.Level, &a.Details, &created); err != nil {
			return nil, err
		}
		a.CreatedAt = parseTime(created)
		out = append(out, a)
	}
	return out, rows.Err()
}

func (s *Store) ListDailySummaries(ctx context.Context, accountID string, limit int) ([]groupmgr.DailySummary, error) {
	if limit <= 0 {
		limit = 100
	}
	rows, err := s.DB.QueryContext(ctx, `SELECT id,account_id,group_id,content,source,created_at FROM daily_summaries WHERE account_id=? ORDER BY id DESC LIMIT ?`, accountID, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := make([]groupmgr.DailySummary, 0)
	for rows.Next() {
		var summary groupmgr.DailySummary
		var created sql.NullString
		if err := rows.Scan(&summary.ID, &summary.AccountID, &summary.GroupID, &summary.Content, &summary.Source, &created); err != nil {
			return nil, err
		}
		summary.CreatedAt = parseTime(created)
		out = append(out, summary)
	}
	return out, rows.Err()
}

func (s *Store) DeleteTask(ctx context.Context, id int64) error {
	_, err := s.DB.ExecContext(ctx, `DELETE FROM tasks WHERE id=?`, id)
	return err
}

package store

import (
	"context"
	"database/sql"
	"errors"
	"time"

	"dh/internal/groupmgr"
)

func (s *Store) GetCardSettings(ctx context.Context, accountID string, groupID int64) (groupmgr.GroupCardSettings, error) {
	var settings groupmgr.GroupCardSettings
	var autoRename, paused, complete int64
	var baseline, snapshot, updated sql.NullString
	err := s.DB.QueryRowContext(ctx, `SELECT account_id,group_id,prefix,auto_rename,paused,baseline_at,last_snapshot_at,reported_count,resolved_count,roster_complete,updated_at FROM group_card_settings WHERE account_id=? AND group_id=?`, accountID, groupID).Scan(&settings.AccountID, &settings.GroupID, &settings.Prefix, &autoRename, &paused, &baseline, &snapshot, &settings.ReportedCount, &settings.ResolvedCount, &complete, &updated)
	if errors.Is(err, sql.ErrNoRows) {
		settings = groupmgr.GroupCardSettings{AccountID: accountID, GroupID: groupID, Prefix: "DH", UpdatedAt: time.Now().UTC()}
		return settings, nil
	}
	settings.AutoRename = intBool(autoRename)
	settings.Paused = intBool(paused)
	settings.RosterComplete = intBool(complete)
	settings.BaselineAt = parseTime(baseline)
	settings.LastSnapshotAt = parseTime(snapshot)
	settings.UpdatedAt = parseTime(updated)
	return settings, err
}

func (s *Store) UpsertCardSettings(ctx context.Context, settings groupmgr.GroupCardSettings) error {
	if settings.Prefix == "" {
		settings.Prefix = "DH"
	}
	_, err := s.DB.ExecContext(ctx, `INSERT INTO group_card_settings(account_id,group_id,prefix,auto_rename,paused,baseline_at,last_snapshot_at,reported_count,resolved_count,roster_complete,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,group_id) DO UPDATE SET prefix=excluded.prefix,auto_rename=excluded.auto_rename,paused=excluded.paused,baseline_at=excluded.baseline_at,last_snapshot_at=excluded.last_snapshot_at,reported_count=excluded.reported_count,resolved_count=excluded.resolved_count,roster_complete=excluded.roster_complete,updated_at=excluded.updated_at`, settings.AccountID, settings.GroupID, settings.Prefix, boolInt(settings.AutoRename), boolInt(settings.Paused), optionalTimeText(settings.BaselineAt), optionalTimeText(settings.LastSnapshotAt), settings.ReportedCount, settings.ResolvedCount, boolInt(settings.RosterComplete), nowText(settings.UpdatedAt))
	return err
}

func (s *Store) EnqueueCardJob(ctx context.Context, job *groupmgr.CardRenameJob) error {
	var id int64
	err := s.DB.QueryRowContext(ctx, `SELECT id FROM card_rename_jobs WHERE account_id=? AND group_id=? AND user_id=? AND state IN ('queued','running','paused') ORDER BY id DESC LIMIT 1`, job.AccountID, job.GroupID, job.UserID).Scan(&id)
	if err == nil {
		job.ID = id
		_, err = s.DB.ExecContext(ctx, `UPDATE card_rename_jobs SET nim_id=?,original_name=?,desired_name=?,suffix=?,source=?,state='queued',attempts=0,next_attempt_at='',last_error='',welcome_pending=?,updated_at=? WHERE id=?`, job.NIMID, job.OriginalName, job.DesiredName, job.Suffix, job.Source, boolInt(job.WelcomePending), nowText(job.UpdatedAt), id)
		return err
	}
	if !errors.Is(err, sql.ErrNoRows) {
		return err
	}
	result, err := s.DB.ExecContext(ctx, `INSERT INTO card_rename_jobs(account_id,group_id,user_id,nim_id,original_name,desired_name,suffix,source,state,attempts,next_attempt_at,last_error,welcome_pending,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)`, job.AccountID, job.GroupID, job.UserID, job.NIMID, job.OriginalName, job.DesiredName, job.Suffix, job.Source, groupmgr.RenameQueued, job.Attempts, optionalTimeText(job.NextAttemptAt), job.LastError, boolInt(job.WelcomePending), nowText(job.CreatedAt), nowText(job.UpdatedAt))
	if err == nil {
		job.ID, _ = result.LastInsertId()
	}
	return err
}

func (s *Store) NextCardJob(ctx context.Context, accountID string, now time.Time, includeAutomatic bool) (groupmgr.CardRenameJob, error) {
	row := s.DB.QueryRowContext(ctx, `SELECT id,account_id,group_id,user_id,nim_id,original_name,desired_name,suffix,source,state,attempts,next_attempt_at,last_error,welcome_pending,created_at,updated_at FROM card_rename_jobs WHERE account_id=? AND (?=1 OR source='baseline') AND ((state='queued' AND (next_attempt_at='' OR next_attempt_at<=?)) OR (state='verified' AND welcome_pending=1 AND (next_attempt_at='' OR next_attempt_at<=?))) ORDER BY group_id,id LIMIT 1`, accountID, boolInt(includeAutomatic), nowText(now), nowText(now))
	return scanCardJob(row)
}

func (s *Store) ListCardJobs(ctx context.Context, accountID string, groupID int64, limit int) ([]groupmgr.CardRenameJob, error) {
	if limit <= 0 {
		limit = 1000
	}
	rows, err := s.DB.QueryContext(ctx, `SELECT id,account_id,group_id,user_id,nim_id,original_name,desired_name,suffix,source,state,attempts,next_attempt_at,last_error,welcome_pending,created_at,updated_at FROM card_rename_jobs WHERE account_id=? AND group_id=? ORDER BY id DESC LIMIT ?`, accountID, groupID, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	out := make([]groupmgr.CardRenameJob, 0)
	for rows.Next() {
		job, err := scanCardJob(rows)
		if err != nil {
			return nil, err
		}
		out = append(out, job)
	}
	return out, rows.Err()
}

func scanCardJob(row scanner) (groupmgr.CardRenameJob, error) {
	var job groupmgr.CardRenameJob
	var next, created, updated sql.NullString
	var welcome int64
	err := row.Scan(&job.ID, &job.AccountID, &job.GroupID, &job.UserID, &job.NIMID, &job.OriginalName, &job.DesiredName, &job.Suffix, &job.Source, &job.State, &job.Attempts, &next, &job.LastError, &welcome, &created, &updated)
	job.NextAttemptAt = parseTime(next)
	job.WelcomePending = intBool(welcome)
	job.CreatedAt = parseTime(created)
	job.UpdatedAt = parseTime(updated)
	return job, err
}

func (s *Store) UpdateCardJob(ctx context.Context, job groupmgr.CardRenameJob) error {
	_, err := s.DB.ExecContext(ctx, `UPDATE card_rename_jobs SET nim_id=?,desired_name=?,suffix=?,state=?,attempts=?,next_attempt_at=?,last_error=?,welcome_pending=?,updated_at=? WHERE id=?`, job.NIMID, job.DesiredName, job.Suffix, job.State, job.Attempts, optionalTimeText(job.NextAttemptAt), job.LastError, boolInt(job.WelcomePending), nowText(job.UpdatedAt), job.ID)
	return err
}

func (s *Store) CompleteCardRename(ctx context.Context, member groupmgr.Member, job groupmgr.CardRenameJob) error {
	tx, err := s.DB.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	defer tx.Rollback()
	_, err = tx.ExecContext(ctx, `UPDATE members SET card_name=?,managed_card_name=?,card_suffix=?,card_status='verified',locked_card_name=?,rename_violations=0,present=1,updated_at=? WHERE account_id=? AND group_id=? AND user_id=?`, job.DesiredName, job.DesiredName, job.Suffix, job.DesiredName, nowText(time.Now()), member.AccountID, member.GroupID, member.UserID)
	if err != nil {
		return err
	}
	_, err = tx.ExecContext(ctx, `UPDATE card_rename_jobs SET state='verified',next_attempt_at='',last_error='',updated_at=? WHERE id=?`, nowText(time.Now()), job.ID)
	if err != nil {
		return err
	}
	return tx.Commit()
}

func (s *Store) RecoverCardJobs(ctx context.Context, accountID string) error {
	now := nowText(time.Now())
	if _, err := s.DB.ExecContext(ctx, `UPDATE card_rename_jobs SET state='queued',next_attempt_at='',updated_at=? WHERE account_id=? AND state='running'`, now, accountID); err != nil {
		return err
	}
	_, err := s.DB.ExecContext(ctx, `UPDATE members SET card_status='queued',updated_at=? WHERE account_id=? AND card_status='running'`, now, accountID)
	return err
}

func (s *Store) RetryFailedCardJobs(ctx context.Context, accountID string, groupID int64) error {
	now := nowText(time.Now())
	if _, err := s.DB.ExecContext(ctx, `UPDATE card_rename_jobs SET state='queued',attempts=0,next_attempt_at='',last_error='',updated_at=? WHERE account_id=? AND group_id=? AND state='failed'`, now, accountID, groupID); err != nil {
		return err
	}
	_, err := s.DB.ExecContext(ctx, `UPDATE members SET card_status='queued',updated_at=? WHERE account_id=? AND group_id=? AND card_status='failed'`, now, accountID, groupID)
	return err
}

func (s *Store) ResumePausedCardJobs(ctx context.Context, accountID string, groupID int64) error {
	now := nowText(time.Now())
	if _, err := s.DB.ExecContext(ctx, `UPDATE card_rename_jobs SET state='queued',next_attempt_at='',last_error='',updated_at=? WHERE account_id=? AND group_id=? AND state='paused'`, now, accountID, groupID); err != nil {
		return err
	}
	_, err := s.DB.ExecContext(ctx, `UPDATE members SET card_status='queued',updated_at=? WHERE account_id=? AND group_id=? AND card_status='planned'`, now, accountID, groupID)
	return err
}

func (s *Store) MarkMemberNoticesRead(ctx context.Context, accountID string, groupID int64) error {
	_, err := s.DB.ExecContext(ctx, `UPDATE members SET notice_read=1 WHERE account_id=? AND group_id=?`, accountID, groupID)
	return err
}

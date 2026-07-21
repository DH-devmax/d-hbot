package store

import (
	"context"
	"database/sql"
	"errors"
	"fmt"
	"strings"
	"time"

	"dh/internal/groupmgr"
)

func (s *Store) UpsertGroupSchedule(ctx context.Context, schedule *groupmgr.GroupSchedule) error {
	if schedule == nil || strings.TrimSpace(schedule.AccountID) == "" {
		return errors.New("群发言计划参数不完整")
	}
	if !validClock(schedule.OpenTime) || !validClock(schedule.CloseTime) || schedule.OpenTime == schedule.CloseTime {
		return errors.New("开群和关群时间需要是不同的 HH:MM")
	}
	if schedule.Timezone == "" {
		schedule.Timezone = "Local"
	}
	if schedule.Name == "" {
		schedule.Name = "每日群发言计划"
	}
	now := nowText(schedule.UpdatedAt)
	if schedule.CreatedAt.IsZero() {
		schedule.CreatedAt = time.Now().UTC()
	}
	if schedule.UpdatedAt.IsZero() {
		schedule.UpdatedAt = time.Now().UTC()
	}
	_, err := s.DB.ExecContext(ctx, `INSERT INTO group_schedules(id,account_id,name,enabled,open_time,close_time,timezone,created_at,updated_at) VALUES(NULL,?,?,?,?,?,?,?,?) ON CONFLICT(account_id,name) DO UPDATE SET enabled=excluded.enabled,open_time=excluded.open_time,close_time=excluded.close_time,timezone=excluded.timezone,updated_at=excluded.updated_at`, schedule.AccountID, schedule.Name, boolInt(schedule.Enabled), schedule.OpenTime, schedule.CloseTime, schedule.Timezone, nowText(schedule.CreatedAt), now)
	if err != nil {
		return fmt.Errorf("保存群发言计划: %w", err)
	}
	if err := s.DB.QueryRowContext(ctx, `SELECT id FROM group_schedules WHERE account_id=? AND name=?`, schedule.AccountID, schedule.Name).Scan(&schedule.ID); err != nil {
		return err
	}
	return nil
}

func (s *Store) ListGroupSchedules(ctx context.Context, accountID string) ([]groupmgr.GroupSchedule, error) {
	rows, err := s.DB.QueryContext(ctx, `SELECT id,account_id,name,enabled,open_time,close_time,timezone,created_at,updated_at FROM group_schedules WHERE account_id=? ORDER BY name,id`, accountID)
	if err != nil {
		return nil, err
	}
	var out []groupmgr.GroupSchedule
	for rows.Next() {
		var schedule groupmgr.GroupSchedule
		var enabled int64
		var created, updated sql.NullString
		if err := rows.Scan(&schedule.ID, &schedule.AccountID, &schedule.Name, &enabled, &schedule.OpenTime, &schedule.CloseTime, &schedule.Timezone, &created, &updated); err != nil {
			return nil, err
		}
		schedule.Enabled = intBool(enabled)
		schedule.CreatedAt, schedule.UpdatedAt = parseTime(created), parseTime(updated)
		out = append(out, schedule)
	}
	rowsErr := rows.Err()
	_ = rows.Close()
	if rowsErr != nil {
		return nil, rowsErr
	}
	// The store intentionally uses one SQLite connection. Load bindings only
	// after closing the schedule cursor so nested reads cannot wait forever.
	for index := range out {
		groups, err := s.ListScheduleGroups(ctx, accountID, out[index].ID)
		if err != nil {
			return nil, err
		}
		for _, group := range groups {
			out[index].GroupIDs = append(out[index].GroupIDs, group.GroupID)
		}
	}
	return out, nil
}

func (s *Store) DeleteGroupSchedule(ctx context.Context, accountID string, id int64) error {
	_, err := s.DB.ExecContext(ctx, `DELETE FROM group_schedules WHERE account_id=? AND id=?`, accountID, id)
	return err
}

func (s *Store) BindScheduleGroups(ctx context.Context, accountID string, scheduleID int64, groupIDs []int64) error {
	tx, err := s.DB.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	rollback := func(err error) error { _ = tx.Rollback(); return err }
	if _, err := tx.ExecContext(ctx, `DELETE FROM group_schedule_groups WHERE account_id=? AND schedule_id=?`, accountID, scheduleID); err != nil {
		return rollback(err)
	}
	now := nowText(time.Now().UTC())
	for _, groupID := range groupIDs {
		if groupID == 0 {
			continue
		}
		if _, err := tx.ExecContext(ctx, `INSERT INTO group_schedule_groups(schedule_id,account_id,group_id,enabled,created_at) VALUES(?,?,?,?,?)`, scheduleID, accountID, groupID, 1, now); err != nil {
			if strings.Contains(strings.ToLower(err.Error()), "unique") {
				return rollback(fmt.Errorf("群已绑定其他启用计划: %d", groupID))
			}
			return rollback(err)
		}
	}
	return tx.Commit()
}

// MoveScheduleGroups assigns the selected groups to one schedule after the UI
// has explicitly confirmed that existing schedule bindings may be replaced.
func (s *Store) MoveScheduleGroups(ctx context.Context, accountID string, scheduleID int64, groupIDs []int64) error {
	tx, err := s.DB.BeginTx(ctx, nil)
	if err != nil {
		return err
	}
	rollback := func(err error) error { _ = tx.Rollback(); return err }
	if _, err := tx.ExecContext(ctx, `DELETE FROM group_schedule_groups WHERE account_id=? AND schedule_id=?`, accountID, scheduleID); err != nil {
		return rollback(err)
	}
	now := nowText(time.Now().UTC())
	for _, groupID := range groupIDs {
		if groupID == 0 {
			continue
		}
		if _, err := tx.ExecContext(ctx, `DELETE FROM group_schedule_groups WHERE account_id=? AND group_id=? AND schedule_id<>?`, accountID, groupID, scheduleID); err != nil {
			return rollback(err)
		}
		if _, err := tx.ExecContext(ctx, `INSERT INTO group_schedule_groups(schedule_id,account_id,group_id,enabled,created_at) VALUES(?,?,?,?,?)`, scheduleID, accountID, groupID, 1, now); err != nil {
			return rollback(err)
		}
	}
	return tx.Commit()
}

func (s *Store) ListScheduleGroups(ctx context.Context, accountID string, scheduleID int64) ([]groupmgr.ScheduleBinding, error) {
	rows, err := s.DB.QueryContext(ctx, `SELECT schedule_id,account_id,group_id,enabled,created_at FROM group_schedule_groups WHERE account_id=? AND schedule_id=? ORDER BY group_id`, accountID, scheduleID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []groupmgr.ScheduleBinding
	for rows.Next() {
		var binding groupmgr.ScheduleBinding
		var enabled int64
		var created sql.NullString
		if err := rows.Scan(&binding.ScheduleID, &binding.AccountID, &binding.GroupID, &enabled, &created); err != nil {
			return nil, err
		}
		binding.Enabled = intBool(enabled)
		binding.CreatedAt = parseTime(created)
		out = append(out, binding)
	}
	return out, rows.Err()
}

// ScheduleGroups returns the group IDs bound to one schedule. It avoids
// exposing the knowledge binding-shaped compatibility model to callers.
func (s *Store) ScheduleGroups(ctx context.Context, accountID string, scheduleID int64) ([]int64, error) {
	rows, err := s.DB.QueryContext(ctx, `SELECT group_id FROM group_schedule_groups WHERE account_id=? AND schedule_id=? AND enabled=1 ORDER BY group_id`, accountID, scheduleID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []int64
	for rows.Next() {
		var groupID int64
		if err := rows.Scan(&groupID); err != nil {
			return nil, err
		}
		out = append(out, groupID)
	}
	return out, rows.Err()
}

func (s *Store) RecordScheduleRun(ctx context.Context, run *groupmgr.ScheduleRun) (bool, error) {
	if run == nil || run.RunKey == "" {
		return false, errors.New("计划执行记录参数不完整")
	}
	created := nowText(run.CreatedAt)
	result, err := s.DB.ExecContext(ctx, `INSERT INTO schedule_runs(schedule_id,account_id,group_id,local_date,action,run_key,success,error,attempts,next_retry_at,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(run_key) DO UPDATE SET attempts=schedule_runs.attempts+1,next_retry_at='',created_at=excluded.created_at WHERE schedule_runs.success=0 AND (schedule_runs.next_retry_at='' OR julianday(schedule_runs.next_retry_at)<=julianday(excluded.created_at))`, run.ScheduleID, run.AccountID, run.GroupID, run.LocalDate, run.Action, run.RunKey, boolInt(run.Success), run.Error, 1, "", created)
	if err != nil {
		return false, err
	}
	count, _ := result.RowsAffected()
	return count == 1, nil
}

func (s *Store) UpdateScheduleRun(ctx context.Context, runKey string, success bool, detail string) error {
	nextRetry := ""
	if !success {
		var attempts int
		if err := s.DB.QueryRowContext(ctx, `SELECT attempts FROM schedule_runs WHERE run_key=?`, runKey).Scan(&attempts); err != nil {
			return err
		}
		delays := []time.Duration{30 * time.Second, 2 * time.Minute, 10 * time.Minute, 30 * time.Minute}
		index := attempts - 1
		if index < 0 {
			index = 0
		}
		if index >= len(delays) {
			index = len(delays) - 1
		}
		nextRetry = nowText(time.Now().UTC().Add(delays[index]))
	}
	_, err := s.DB.ExecContext(ctx, `UPDATE schedule_runs SET success=?,error=?,next_retry_at=? WHERE run_key=?`, boolInt(success), detail, nextRetry, runKey)
	return err
}

func (s *Store) DeleteScheduleRun(ctx context.Context, runKey string) error {
	_, err := s.DB.ExecContext(ctx, `DELETE FROM schedule_runs WHERE run_key=?`, runKey)
	return err
}

func validClock(value string) bool {
	parsed, err := time.Parse("15:04", strings.TrimSpace(value))
	return err == nil && parsed.Hour() >= 0 && parsed.Hour() <= 23 && parsed.Minute() >= 0 && parsed.Minute() <= 59
}

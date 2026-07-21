package store

import (
	"context"
	"strings"
	"testing"
	"time"

	"dh/internal/groupmgr"
)

func TestScheduleBindingRequiresExplicitMove(t *testing.T) {
	s := openTestStore(t)
	ctx := context.Background()
	first := groupmgr.GroupSchedule{AccountID: "acct", Name: "白班", OpenTime: "08:00", CloseTime: "18:00", Enabled: true, UpdatedAt: time.Now().UTC()}
	second := groupmgr.GroupSchedule{AccountID: "acct", Name: "夜班", OpenTime: "18:00", CloseTime: "08:00", Enabled: true, UpdatedAt: time.Now().UTC()}
	if err := s.UpsertGroupSchedule(ctx, &first); err != nil {
		t.Fatal(err)
	}
	if err := s.UpsertGroupSchedule(ctx, &second); err != nil {
		t.Fatal(err)
	}
	if err := s.BindScheduleGroups(ctx, "acct", first.ID, []int64{100}); err != nil {
		t.Fatal(err)
	}
	if err := s.BindScheduleGroups(ctx, "acct", second.ID, []int64{100}); err == nil || !strings.Contains(err.Error(), "群已绑定其他启用计划") {
		t.Fatalf("expected binding conflict, got %v", err)
	}
	if err := s.MoveScheduleGroups(ctx, "acct", second.ID, []int64{100}); err != nil {
		t.Fatal(err)
	}
	firstGroups, err := s.ScheduleGroups(ctx, "acct", first.ID)
	if err != nil {
		t.Fatal(err)
	}
	secondGroups, err := s.ScheduleGroups(ctx, "acct", second.ID)
	if err != nil {
		t.Fatal(err)
	}
	if len(firstGroups) != 0 || len(secondGroups) != 1 || secondGroups[0] != 100 {
		t.Fatalf("unexpected bindings: first=%v second=%v", firstGroups, secondGroups)
	}
}

func TestScheduleRunRetriesOnlyAfterBackoff(t *testing.T) {
	s := openTestStore(t)
	ctx := context.Background()
	schedule := groupmgr.GroupSchedule{AccountID: "acct", Name: "每日", OpenTime: "08:00", CloseTime: "22:00", Enabled: true, UpdatedAt: time.Now().UTC()}
	if err := s.UpsertGroupSchedule(ctx, &schedule); err != nil {
		t.Fatal(err)
	}
	run := groupmgr.ScheduleRun{ScheduleID: schedule.ID, AccountID: "acct", GroupID: 100, LocalDate: "2026-07-20", Action: groupmgr.ActionGroupMute, RunKey: "acct:daily:100", CreatedAt: time.Now().UTC()}
	claimed, err := s.RecordScheduleRun(ctx, &run)
	if err != nil || !claimed {
		t.Fatalf("first claim=%v err=%v", claimed, err)
	}
	if err := s.UpdateScheduleRun(ctx, run.RunKey, false, "temporary"); err != nil {
		t.Fatal(err)
	}
	claimed, err = s.RecordScheduleRun(ctx, &run)
	if err != nil || claimed {
		t.Fatalf("immediate retry claim=%v err=%v", claimed, err)
	}
	if _, err := s.DB.ExecContext(ctx, `UPDATE schedule_runs SET next_retry_at=? WHERE run_key=?`, nowText(time.Now().UTC().Add(-time.Second)), run.RunKey); err != nil {
		t.Fatal(err)
	}
	run.CreatedAt = time.Now().UTC()
	claimed, err = s.RecordScheduleRun(ctx, &run)
	if err != nil || !claimed {
		t.Fatalf("due retry claim=%v err=%v", claimed, err)
	}
}

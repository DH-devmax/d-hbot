package store

import (
	"context"
	"path/filepath"
	"testing"

	"dh/internal/groupmgr"
)

func TestKnowledgeBaseManyToManyAndDefaultIsolation(t *testing.T) {
	s, err := Open(filepath.Join(t.TempDir(), "dh.db"))
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	ctx := context.Background()
	if err := s.UpsertGroup(ctx, groupmgr.Group{AccountID: "a", GroupID: 1, Name: "一群"}); err != nil {
		t.Fatal(err)
	}
	if _, err := s.EnsureDefaultKnowledgeBase(ctx, "a"); err != nil {
		t.Fatal(err)
	}
	baseA := groupmgr.KnowledgeBase{AccountID: "a", Name: "群规", Enabled: true}
	baseB := groupmgr.KnowledgeBase{AccountID: "a", Name: "FAQ", Enabled: true}
	if err := s.CreateKnowledgeBase(ctx, &baseA); err != nil {
		t.Fatal(err)
	}
	if err := s.CreateKnowledgeBase(ctx, &baseB); err != nil {
		t.Fatal(err)
	}
	for _, base := range []*groupmgr.KnowledgeBase{&baseA, &baseB} {
		if err := s.UpsertKnowledgeDocument(ctx, &groupmgr.KnowledgeDocument{BaseID: base.ID, Title: base.Name, Content: base.Name + " 内容"}); err != nil {
			t.Fatal(err)
		}
		if err := s.BindKnowledgeBaseGroups(ctx, "a", base.ID, []int64{1}); err != nil {
			t.Fatal(err)
		}
	}
	docs, err := s.ListKnowledgeForGroup(ctx, "a", 1)
	if err != nil || len(docs) != 2 {
		t.Fatalf("docs=%d err=%v", len(docs), err)
	}
	other, err := s.ListKnowledgeForGroup(ctx, "a", 2)
	if err != nil || len(other) != 0 {
		t.Fatalf("unbound docs=%d err=%v", len(other), err)
	}
	defaultBases, err := s.ListKnowledgeBases(ctx, "a")
	if err != nil || len(defaultBases) != 3 {
		t.Fatalf("bases=%d err=%v", len(defaultBases), err)
	}
}

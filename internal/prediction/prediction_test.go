package prediction

import (
	"context"
	"testing"
	"time"
)

type fixtureSource struct{}

func (fixtureSource) ListGames(context.Context) ([]Game, error) {
	return []Game{{ID: "fixture", Name: "测试彩种", Alias: []string{"测试"}}, {ID: "fixture2", Name: "第二彩种"}}, nil
}
func (fixtureSource) FetchLive(context.Context, Game) (Snapshot, error) {
	return Snapshot{Game: Game{ID: "fixture", Name: "测试彩种"}, Period: "20260720001", Result: []int{1, 2, 3}, UpdatedAt: time.Date(2026, 7, 20, 12, 0, 0, 0, time.Local), DataStatus: "已整理"}, nil
}
func (fixtureSource) FetchHistory(context.Context, Game, int) ([][]int, error) {
	return [][]int{{1, 2, 3}}, nil
}

func TestPredictionIntentAndNormalizedPrompt(t *testing.T) {
	if !IsRequest("@ DH 预测 测试") || IsRequest("普通消息 预测") {
		t.Fatal("prediction trigger mismatch")
	}
	service := NewWithSource(fixtureSource{})
	snapshot, promptText, err := service.Build(context.Background(), "@DH 预测 测试")
	if err != nil || snapshot.Period != "20260720001" || promptText == "" {
		t.Fatalf("snapshot=%+v prompt=%q err=%v", snapshot, promptText, err)
	}
	if got := Fallback(snapshot); got == "" || containsAny(got, "endpoint", "openCode") {
		t.Fatalf("fallback leaked raw fields: %q", got)
	}
}

func TestPredictionUnknownGameReturnsChoice(t *testing.T) {
	service := NewWithSource(fixtureSource{})
	snapshot, promptText, err := service.Build(context.Background(), "@DH 预测")
	if err != nil || snapshot.Game.ID != "" || promptText == "" {
		t.Fatalf("snapshot=%+v prompt=%q err=%v", snapshot, promptText, err)
	}
}

func TestAnalyzeBuildsStableNormalizedStatistics(t *testing.T) {
	history := [][]int{{1, 2, 3}, {1, 4, 5}, {1, 2, 6}, {7, 8, 9}, {1, 2, 0}}
	stats := Analyze(history)
	if stats.Window != 5 || len(stats.Hot) != 3 || stats.Hot[0] != 1 || len(stats.Candidates) != 3 {
		t.Fatalf("unexpected stats: %+v", stats)
	}
	if stats.Trend == "" || stats.Confidence == "" {
		t.Fatalf("missing normalized explanation: %+v", stats)
	}
}

func containsAny(value string, terms ...string) bool {
	for _, term := range terms {
		for i := 0; i+len(term) <= len(value); i++ {
			if value[i:i+len(term)] == term {
				return true
			}
		}
	}
	return false
}

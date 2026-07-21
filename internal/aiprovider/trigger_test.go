package aiprovider

import "testing"

func TestShouldReplyOnlyExplicitDHMention(t *testing.T) {
	for _, test := range []struct {
		text string
		want bool
	}{
		{"普通聊天 DH 在吗", false},
		{"@DH 你好", true},
		{"@ DH 预测", true},
		{"@DH_bot 你好", false},
		{"@DH预测", true},
	} {
		if got := ShouldReply(test.text, nil); got != test.want {
			t.Fatalf("ShouldReply(%q)=%v, want %v", test.text, got, test.want)
		}
	}
}

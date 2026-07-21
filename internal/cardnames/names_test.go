package cardnames

import "testing"

func TestCandidates(t *testing.T) {
	tests := map[string]string{
		"广州校长":   "广校",
		"王小明":    "王明",
		"张三":     "张三",
		"A-lice": "Ae",
	}
	for input, want := range tests {
		got := Candidates(input)
		if len(got) == 0 || got[0] != want {
			t.Fatalf("Candidates(%q)=%v, want first %q", input, got, want)
		}
	}
	for _, input := range []string{"", "1", ".", "1234", "😀"} {
		if got := Candidates(input); len(got) != 0 {
			t.Fatalf("Candidates(%q)=%v, want empty", input, got)
		}
	}
}

func TestSuffixBoundaries(t *testing.T) {
	tests := map[int]string{
		1: "0001", 9999: "9999", 10000: "a001", 35973: "z999",
		35974: "aa01", 102897: "zz99", 102898: "aaa1", Capacity: "zzz9",
	}
	for input, want := range tests {
		got, err := Suffix(input)
		if err != nil || got != want {
			t.Fatalf("Suffix(%d)=%q,%v want %q", input, got, err, want)
		}
	}
	if _, err := Suffix(Capacity + 1); err == nil {
		t.Fatal("capacity overflow accepted")
	}
}

func TestFirstAvailable(t *testing.T) {
	got := FirstAvailable(Candidates("广州校长"), map[string]bool{"广校": true})
	if got == "" || got == "广校" {
		t.Fatalf("collision fallback=%q", got)
	}
}

func TestExtractSuffix(t *testing.T) {
	for _, value := range []string{"DH群员0001", "前缀群员a001", "DH群员zz99", "DH群员zzz9"} {
		if got := ExtractSuffix(value); len([]rune(got)) != 4 {
			t.Fatalf("ExtractSuffix(%q)=%q", value, got)
		}
	}
	if got := ExtractSuffix("广州校长"); got != "" {
		t.Fatalf("unexpected suffix %q", got)
	}
	for _, invalid := range []string{"a0b1", "0000", "aaaa", "A001"} {
		if ValidSuffix(invalid) {
			t.Fatalf("invalid suffix accepted: %q", invalid)
		}
	}
}

package moderation

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
)

// ExportRules emits a stable, human-readable rules document for UI export and
// migration tooling.
func ExportRules(rules []ModerationRule) ([]byte, error) {
	return json.MarshalIndent(rules, "", "  ")
}

// ImportRules uses strict decoding, validates every rule, and returns the same
// priority order used by Engine.
func ImportRules(data []byte) ([]ModerationRule, error) {
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	var rules []ModerationRule
	if err := decoder.Decode(&rules); err != nil {
		return nil, fmt.Errorf("解析群管规则 JSON 失败：%w", err)
	}
	var trailing any
	if err := decoder.Decode(&trailing); err != io.EOF {
		return nil, fmt.Errorf("群管规则 JSON 末尾包含多余数据")
	}
	engine, err := NewValidatedEngine(rules)
	if err != nil {
		return nil, err
	}
	return engine.Rules(), nil
}

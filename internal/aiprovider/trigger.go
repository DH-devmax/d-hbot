package aiprovider

import "strings"

func ShouldReply(text string, wakeWords []string) bool {
	_ = wakeWords
	trimmed := strings.TrimSpace(text)
	if trimmed == "" {
		return false
	}
	// AI replies are intentionally explicit: only a real @DH mention wakes it.
	// This avoids answering ordinary conversation, question-shaped text, or a
	// configured nickname that happens to occur in a message.
	lower := strings.ReplaceAll(strings.ToLower(trimmed), "@ dh", "@dh")
	for offset := 0; ; {
		index := strings.Index(lower[offset:], "@dh")
		if index < 0 {
			return false
		}
		index += offset
		end := index + len("@dh")
		//旺商聊有时会把提及文本序列化为“@ DH”。两种形式都
		//只在明确的 DH 提及下唤醒，避免普通聊天误触发。
		if end < len(lower) && lower[end] == ' ' {
			end++
			for end < len(lower) && lower[end] == ' ' {
				end++
			}
		}
		if end == len(lower) {
			return true
		}
		next := lower[end]
		if !((next >= 'a' && next <= 'z') || (next >= '0' && next <= '9') || next == '_') {
			return true
		}
		offset = end
	}
}

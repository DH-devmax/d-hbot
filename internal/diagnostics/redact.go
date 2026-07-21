package diagnostics

import "regexp"

var sensitivePattern = regexp.MustCompile(`(?i)(api[_-]?key|token|authorization|cookie)(["']?\s*[:=]\s*["']?)([^"',\s]+)`)

// Redact removes credential values while preserving enough context to diagnose
// which setting was involved.
func Redact(value string) string {
	return sensitivePattern.ReplaceAllString(value, `$1$2[redacted]`)
}

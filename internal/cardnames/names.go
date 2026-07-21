package cardnames

import (
	"errors"
	"fmt"
	"sort"
	"strings"
	"unicode"
)

const Capacity = 261081

var ErrCapacity = errors.New("群名片固定四位编号已用完")

func Normalize(value string) []rune {
	out := make([]rune, 0, len([]rune(value)))
	for _, current := range []rune(strings.TrimSpace(value)) {
		if unicode.Is(unicode.Han, current) || unicode.IsLetter(current) || unicode.IsDigit(current) {
			out = append(out, current)
		}
	}
	return out
}

func Missing(value string) bool {
	trimmed := strings.TrimSpace(value)
	if trimmed == "" || trimmed == "1" || trimmed == "." {
		return true
	}
	cleaned := Normalize(trimmed)
	if len(cleaned) < 2 {
		return true
	}
	allDigits := true
	for _, current := range cleaned {
		if !unicode.IsDigit(current) {
			allDigits = false
			break
		}
	}
	return allDigits
}

// Candidates returns deterministic two-rune names. The first candidate follows
// the configured abbreviation rule; later candidates resolve collisions.
func Candidates(value string) []string {
	cleaned := Normalize(value)
	if len(cleaned) < 2 || Missing(value) {
		return nil
	}
	if len(cleaned) == 2 {
		return []string{string(cleaned)}
	}
	allHan := true
	for _, current := range cleaned {
		if !unicode.Is(unicode.Han, current) {
			allHan = false
			break
		}
	}
	primarySecond := len(cleaned) - 1
	if allHan && len(cleaned) >= 4 {
		primarySecond = len(cleaned) / 2
	}
	indices := make([][2]int, 0, len(cleaned)*len(cleaned))
	indices = append(indices, [2]int{0, primarySecond})
	if allHan && len(cleaned) >= 4 {
		middle := len(cleaned) / 2
		for left := 0; left < middle; left++ {
			for right := middle; right < len(cleaned); right++ {
				indices = append(indices, [2]int{left, right})
			}
		}
	}
	for left := 0; left < len(cleaned)-1; left++ {
		for right := left + 1; right < len(cleaned); right++ {
			indices = append(indices, [2]int{left, right})
		}
	}
	seen := make(map[string]bool)
	out := make([]string, 0, len(indices))
	for _, pair := range indices {
		candidate := string([]rune{cleaned[pair[0]], cleaned[pair[1]]})
		if seen[candidate] {
			continue
		}
		seen[candidate] = true
		out = append(out, candidate)
	}
	return out
}

func Suffix(index int) (string, error) {
	if index < 1 || index > Capacity {
		return "", ErrCapacity
	}
	switch {
	case index <= 9999:
		return fmt.Sprintf("%04d", index), nil
	case index <= 35973:
		offset := index - 10000
		return fmt.Sprintf("%c%03d", 'a'+rune(offset/999), offset%999+1), nil
	case index <= 102897:
		offset := index - 35974
		letters := offset / 99
		return fmt.Sprintf("%c%c%02d", 'a'+rune(letters/26), 'a'+rune(letters%26), offset%99+1), nil
	default:
		offset := index - 102898
		letters := offset / 9
		return fmt.Sprintf("%c%c%c%d", 'a'+rune(letters/(26*26)), 'a'+rune((letters/26)%26), 'a'+rune(letters%26), offset%9+1), nil
	}
}

func NumberedName(prefix string, index int) (string, string, error) {
	suffix, err := Suffix(index)
	if err != nil {
		return "", "", err
	}
	prefix = strings.TrimSpace(prefix)
	if prefix == "" {
		prefix = "DH"
	}
	return prefix + "群员" + suffix, suffix, nil
}

func ValidSuffix(value string) bool {
	runes := []rune(value)
	if len(runes) != 4 {
		return false
	}
	letters := 0
	for letters < len(runes) && runes[letters] >= 'a' && runes[letters] <= 'z' {
		letters++
	}
	if letters > 3 {
		return false
	}
	number := 0
	for index := letters; index < len(runes); index++ {
		if !unicode.IsDigit(runes[index]) {
			return false
		}
		number = number*10 + int(runes[index]-'0')
	}
	return number > 0
}

func ExtractSuffix(value string) string {
	runes := []rune(strings.TrimSpace(value))
	if len(runes) < 6 || string(runes[len(runes)-6:len(runes)-4]) != "群员" {
		return ""
	}
	candidate := string(runes[len(runes)-4:])
	if ValidSuffix(candidate) {
		return candidate
	}
	return ""
}

func FirstAvailable(candidates []string, used map[string]bool) string {
	for _, candidate := range candidates {
		if !used[candidate] {
			return candidate
		}
	}
	return ""
}

func SortMembersStable[T any](items []T, discovered func(T) int64, userID func(T) int64) {
	sort.SliceStable(items, func(left, right int) bool {
		leftTime, rightTime := discovered(items[left]), discovered(items[right])
		if leftTime != rightTime {
			return leftTime < rightTime
		}
		return userID(items[left]) < userID(items[right])
	})
}

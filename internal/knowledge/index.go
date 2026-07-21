package knowledge

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"unicode"
	"unicode/utf8"
)

const maxDocumentSize = 4 << 20

type Document struct {
	ID      string
	GroupID int64
	Title   string
	Source  string
	Text    string
}

type Chunk struct {
	DocumentID string
	GroupID    int64
	Title      string
	Source     string
	Index      int
	Text       string
	tokens     map[string]int
}

type Result struct {
	Chunk Chunk
	Score float64
}

type Index struct{ chunks []Chunk }

func LoadDocument(path string, groupID int64) (Document, error) {
	extension := strings.ToLower(filepath.Ext(path))
	if extension != ".txt" && extension != ".md" && extension != ".markdown" {
		return Document{}, fmt.Errorf("不支持的知识库文档格式 %q", extension)
	}
	info, err := os.Stat(path)
	if err != nil {
		return Document{}, err
	}
	if info.Size() > maxDocumentSize {
		return Document{}, fmt.Errorf("知识库文档超过 %d 字节", maxDocumentSize)
	}
	raw, err := os.ReadFile(path)
	if err != nil {
		return Document{}, err
	}
	if !utf8.Valid(raw) {
		return Document{}, fmt.Errorf("知识库文档必须使用 UTF-8 编码")
	}
	return Document{ID: filepath.Base(path), GroupID: groupID, Title: strings.TrimSuffix(filepath.Base(path), extension), Source: path, Text: string(raw)}, nil
}

func (index *Index) Add(document Document) {
	for position, text := range split(document.Text, 800, 120) {
		index.chunks = append(index.chunks, Chunk{
			DocumentID: document.ID, GroupID: document.GroupID, Title: document.Title,
			Source: document.Source, Index: position, Text: text, tokens: tokenize(text),
		})
	}
}

func (index *Index) Replace(documents []Document) {
	index.chunks = nil
	for _, document := range documents {
		index.Add(document)
	}
}

func (index *Index) Search(groupID int64, query string, limit int) []Result {
	if limit <= 0 {
		limit = 6
	}
	queryTokens := tokenize(query)
	results := make([]Result, 0)
	for _, chunk := range index.chunks {
		if chunk.GroupID != 0 && chunk.GroupID != groupID {
			continue
		}
		score := 0.0
		for token, queryCount := range queryTokens {
			if count := chunk.tokens[token]; count > 0 {
				score += float64(min(queryCount, count))
				if len([]rune(token)) > 2 {
					score += 1.5
				}
			}
		}
		if score > 0 {
			results = append(results, Result{Chunk: chunk, Score: score})
		}
	}
	sort.SliceStable(results, func(left, right int) bool { return results[left].Score > results[right].Score })
	if len(results) > limit {
		results = results[:limit]
	}
	return results
}

func split(text string, size, overlap int) []string {
	runes := []rune(strings.TrimSpace(text))
	if len(runes) == 0 {
		return nil
	}
	var chunks []string
	for start := 0; start < len(runes); {
		end := min(start+size, len(runes))
		if end < len(runes) {
			for cursor := end; cursor > start+size/2; cursor-- {
				if runes[cursor-1] == '\n' || runes[cursor-1] == '。' {
					end = cursor
					break
				}
			}
		}
		chunks = append(chunks, strings.TrimSpace(string(runes[start:end])))
		if end == len(runes) {
			break
		}
		start = max(end-overlap, start+1)
	}
	return chunks
}

func tokenize(text string) map[string]int {
	normalized := []rune(strings.ToLower(text))
	result := make(map[string]int)
	words := strings.FieldsFunc(string(normalized), func(r rune) bool {
		return unicode.IsSpace(r) || unicode.IsPunct(r) || unicode.IsSymbol(r)
	})
	for _, word := range words {
		wordRunes := []rune(word)
		if len(wordRunes) <= 3 {
			result[word]++
		}
		for index := 0; index+1 < len(wordRunes); index++ {
			result[string(wordRunes[index:index+2])]++
		}
		for index := 0; index+2 < len(wordRunes); index++ {
			result[string(wordRunes[index:index+3])]++
		}
	}
	return result
}

package knowledge

import "testing"

func TestChineseKnowledgeSearch(t *testing.T) {
	var index Index
	index.Add(Document{ID: "rules", GroupID: 10, Title: "群规", Text: "群内发布招聘广告前需要联系管理员。禁止连续刷屏。"})
	index.Add(Document{ID: "other", GroupID: 20, Title: "其他群", Text: "这是其他群的内容。"})
	results := index.Search(10, "招聘广告要联系谁", 3)
	if len(results) == 0 || results[0].Chunk.DocumentID != "rules" {
		t.Fatalf("unexpected results: %+v", results)
	}
	for _, result := range results {
		if result.Chunk.GroupID == 20 {
			t.Fatal("cross-group knowledge leaked")
		}
	}
}

func TestSplitOverlap(t *testing.T) {
	chunks := split("第一段。第二段。第三段。", 6, 2)
	if len(chunks) < 2 {
		t.Fatalf("expected multiple chunks: %v", chunks)
	}
}

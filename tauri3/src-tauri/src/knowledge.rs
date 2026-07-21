use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::models::KnowledgeDocument;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeHit {
    pub document: KnowledgeDocument,
    pub score: f64,
    pub excerpt: String,
    pub chunk_index: usize,
    pub chunk_hash: String,
}

pub fn search(query: &str, documents: &[KnowledgeDocument], limit: usize) -> Vec<KnowledgeHit> {
    search_top_k(query, documents, limit)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeChunk {
    pub document_id: i64,
    pub index: usize,
    pub text: String,
    pub content_hash: String,
}

pub fn content_hash(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn chunk_text(content: &str, max_chars: usize, overlap_chars: usize) -> Vec<String> {
    let chars = content.trim().chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return Vec::new();
    }
    let max_chars = max_chars.max(1);
    let overlap_chars = overlap_chars.min(max_chars.saturating_sub(1));
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = (start + max_chars).min(chars.len());
        if end < chars.len() {
            let earliest_break = start + max_chars / 2;
            for cursor in (earliest_break.max(start + 1)..=end).rev() {
                if matches!(chars[cursor - 1], '\n' | '。' | '！' | '？') {
                    end = cursor;
                    break;
                }
            }
        }
        let chunk = chars[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_string();
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
        if end == chars.len() {
            break;
        }
        start = end.saturating_sub(overlap_chars).max(start + 1);
    }
    chunks
}

pub fn chunk_document(
    document: &KnowledgeDocument,
    max_chars: usize,
    overlap_chars: usize,
) -> Vec<KnowledgeChunk> {
    chunk_text(&document.content, max_chars, overlap_chars)
        .into_iter()
        .enumerate()
        .map(|(index, text)| KnowledgeChunk {
            document_id: document.id,
            index,
            content_hash: content_hash(&text),
            text,
        })
        .collect()
}

pub fn search_top_k(
    query: &str,
    documents: &[KnowledgeDocument],
    limit: usize,
) -> Vec<KnowledgeHit> {
    if limit == 0 {
        return Vec::new();
    }
    let query_terms = terms(query);
    if query_terms.is_empty() {
        return Vec::new();
    }
    let mut unique = HashSet::new();
    let mut hits = Vec::new();
    for document in documents {
        let unique_key = if document.content_hash.is_empty() {
            content_hash(&document.content)
        } else {
            document.content_hash.clone()
        };
        if !unique.insert(unique_key) {
            continue;
        }
        let title_terms = terms(&document.title);
        for chunk in chunk_document(document, 800, 120) {
            let body_terms = terms(&chunk.text);
            let mut score = 0.0;
            for (term, query_count) in &query_terms {
                if title_terms.contains_key(term) {
                    score += 3.0;
                }
                if let Some(count) = body_terms.get(term) {
                    score += (*query_count).min(*count) as f64;
                    score += 1.0 + (*count as f64).ln_1p();
                }
            }
            if score > 0.0 {
                hits.push(KnowledgeHit {
                    document: document.clone(),
                    score,
                    excerpt: excerpt(&chunk.text, query, 260),
                    chunk_index: chunk.index,
                    chunk_hash: chunk.content_hash,
                });
            }
        }
    }
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then(left.document.id.cmp(&right.document.id))
            .then(left.chunk_index.cmp(&right.chunk_index))
    });
    hits.truncate(limit);
    hits
}

fn terms(value: &str) -> HashMap<String, usize> {
    let normalized = value.to_lowercase();
    let chars = normalized
        .chars()
        .filter(|character| !character.is_whitespace() && !character.is_ascii_punctuation())
        .collect::<Vec<_>>();
    let mut result = HashMap::new();
    for token in normalized
        .split(|character: char| character.is_whitespace() || character.is_ascii_punctuation())
        .filter(|token| token.len() >= 2)
    {
        *result.entry(token.to_string()).or_default() += 1;
    }
    for width in [2, 3] {
        for window in chars.windows(width) {
            *result.entry(window.iter().collect()).or_default() += 1;
        }
    }
    result
}

fn excerpt(content: &str, query: &str, max_chars: usize) -> String {
    let query_first = query.chars().find(|character| !character.is_whitespace());
    let chars = content.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return content.to_string();
    }
    let center = query_first
        .and_then(|needle| chars.iter().position(|character| *character == needle))
        .unwrap_or(0);
    let start = center
        .saturating_sub(max_chars / 4)
        .min(chars.len().saturating_sub(max_chars));
    chars[start..start + max_chars].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document(id: i64, title: &str, content: &str, hash: &str) -> KnowledgeDocument {
        KnowledgeDocument {
            id,
            base_id: 1,
            base_name: "FAQ".into(),
            title: title.into(),
            kind: "text".into(),
            content: content.into(),
            source: "test".into(),
            content_hash: hash.into(),
            enabled: true,
        }
    }
    #[test]
    fn title_and_chinese_ngrams_rank_relevant_documents() {
        let result = search(
            "怎么修改群名片",
            &[
                document(1, "群名片", "管理员可以修改群名片。", "1"),
                document(2, "欢迎语", "新成员会收到欢迎。", "2"),
            ],
            2,
        );
        assert_eq!(result[0].document.id, 1);
    }
    #[test]
    fn duplicate_content_hash_is_sent_once() {
        assert_eq!(
            search(
                "群规",
                &[
                    document(1, "群规", "文明交流", "same"),
                    document(2, "群规副本", "文明交流", "same")
                ],
                5
            )
            .len(),
            1
        );
    }

    #[test]
    fn duplicate_content_without_stored_hash_is_still_sent_once() {
        let content = "资金、账号和验证码需要人工核实";
        let result = search(
            "验证码核实",
            &[
                document(1, "FAQ A", content, ""),
                document(2, "FAQ B", content, ""),
            ],
            10,
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].document.id, 1);
    }

    #[test]
    fn chunk_hashes_are_stable_and_document_scoped() {
        let first = document(
            7,
            "群规",
            "文明交流。涉及资金时请人工核实。不发布恶意链接。",
            "",
        );
        let chunks = chunk_document(&first, 10, 2);
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|chunk| chunk.document_id == 7));
        assert_eq!(chunks, chunk_document(&first, 10, 2));
        assert!(chunks
            .iter()
            .all(|chunk| chunk.content_hash == content_hash(&chunk.text)));
    }

    #[test]
    fn unrelated_documents_do_not_leak_into_results() {
        let result = search(
            "群名片",
            &[
                document(1, "群名片", "管理员可以修改群名片", "a"),
                document(2, "资金", "转账需要人工核实", "b"),
            ],
            10,
        );
        assert_eq!(
            result.iter().map(|hit| hit.document.id).collect::<Vec<_>>(),
            vec![1]
        );
    }
}

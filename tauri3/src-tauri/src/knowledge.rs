use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::models::KnowledgeDocument;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeHit {
    pub document: KnowledgeDocument,
    pub score: f64,
    pub excerpt: String,
}

pub fn search(query: &str, documents: &[KnowledgeDocument], limit: usize) -> Vec<KnowledgeHit> {
    let query_terms = terms(query);
    if query_terms.is_empty() {
        return Vec::new();
    }
    let mut unique = HashSet::new();
    let mut hits = Vec::new();
    for document in documents {
        let unique_key = if document.content_hash.is_empty() {
            document.content.clone()
        } else {
            document.content_hash.clone()
        };
        if !unique.insert(unique_key) {
            continue;
        }
        let title_terms = terms(&document.title);
        let body_terms = terms(&document.content);
        let mut score = 0.0;
        for term in query_terms.keys() {
            if title_terms.contains_key(term) {
                score += 3.0;
            }
            if let Some(count) = body_terms.get(term) {
                score += 1.0 + (*count as f64).ln_1p();
            }
        }
        if score > 0.0 {
            hits.push(KnowledgeHit {
                document: document.clone(),
                score,
                excerpt: excerpt(&document.content, query, 260),
            });
        }
    }
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then(left.document.id.cmp(&right.document.id))
    });
    hits.truncate(limit.max(1));
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
}

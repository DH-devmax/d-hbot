//! 知识库、文档、分块与群绑定。

use tauri::State;

use crate::*;

#[tauri::command]
pub(crate) async fn list_knowledge_bases(
    state: State<'_, AppState>,
    account_id: String,
) -> AppResult<Vec<KnowledgeBase>> {
    state
        .database_executor
        .list_knowledge_bases(account_id)
        .await
}

#[tauri::command]
pub(crate) async fn create_knowledge_base(state: State<'_, AppState>, base: KnowledgeBase) -> AppResult<i64> {
    require_account(&state, &base.account_id).await?;
    state.database_executor.create_knowledge_base(base).await
}

#[tauri::command]
pub(crate) async fn update_knowledge_base(state: State<'_, AppState>, base: KnowledgeBase) -> AppResult<()> {
    require_account(&state, &base.account_id).await?;
    state.database_executor.update_knowledge_base(base).await
}

#[tauri::command]
pub(crate) async fn clone_knowledge_base(
    state: State<'_, AppState>,
    account_id: String,
    base_id: i64,
    name: String,
) -> AppResult<i64> {
    require_account(&state, &account_id).await?;
    state
        .database_executor
        .clone_knowledge_base(account_id, base_id, name)
        .await
}

#[tauri::command]
pub(crate) async fn delete_knowledge_base(
    state: State<'_, AppState>,
    account_id: String,
    base_id: i64,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    state
        .database_executor
        .delete_knowledge_base(account_id, base_id)
        .await
}

#[tauri::command]
pub(crate) async fn list_knowledge_documents(
    state: State<'_, AppState>,
    base_id: i64,
) -> AppResult<Vec<KnowledgeDocument>> {
    state
        .database_executor
        .list_knowledge_documents(base_id)
        .await
}

#[tauri::command]
pub(crate) async fn save_knowledge_document(
    state: State<'_, AppState>,
    document: KnowledgeDocument,
) -> AppResult<i64> {
    let account_id = state
        .database_executor
        .knowledge_base_account(document.base_id)
        .await?;
    require_account(&state, &account_id).await?;
    let document_id = state
        .database_executor
        .upsert_knowledge_document(document.clone())
        .await?;
    let mut stored_document = document;
    stored_document.id = document_id;
    let chunks = knowledge::chunk_document(&stored_document, 800, 120)
        .into_iter()
        .map(|chunk| StoredKnowledgeChunk {
            id: 0,
            document_id,
            chunk_index: chunk.index as i64,
            token_count: chunk.text.chars().count() as i64,
            content: chunk.text,
            content_hash: chunk.content_hash,
            enabled: stored_document.enabled,
        })
        .collect::<Vec<_>>();
    state
        .database_executor
        .replace_knowledge_chunks(document_id, chunks)
        .await?;
    Ok(document_id)
}

#[tauri::command]
pub(crate) async fn delete_knowledge_document(
    state: State<'_, AppState>,
    base_id: i64,
    document_id: i64,
) -> AppResult<()> {
    let account_id = state
        .database_executor
        .knowledge_base_account(base_id)
        .await?;
    require_account(&state, &account_id).await?;
    state
        .database_executor
        .delete_knowledge_document(base_id, document_id)
        .await
}

#[tauri::command]
pub(crate) async fn bind_knowledge_base(
    state: State<'_, AppState>,
    base_id: i64,
    account_id: String,
    group_ids: Vec<i64>,
) -> AppResult<()> {
    require_account(&state, &account_id).await?;
    for group_id in &group_ids {
        require_manager(&state, *group_id).await?;
    }
    state
        .database_executor
        .bind_knowledge_base(account_id, base_id, group_ids)
        .await
}

#[tauri::command]
pub(crate) async fn list_knowledge_bindings(
    state: State<'_, AppState>,
    account_id: String,
    base_id: Option<i64>,
) -> AppResult<Vec<KnowledgeBinding>> {
    state
        .database_executor
        .list_knowledge_bindings(account_id, base_id)
        .await
}

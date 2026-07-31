#![cfg(all(target_os = "macos", feature = "fixture"))]

use std::time::{Duration, Instant};

use dh_bot_lib::models::{KnowledgeBase, KnowledgeChunk, KnowledgeDocument};
use dh_bot_lib::{
    AiProviderFactory, ConfiguredAiProviderFactory, FixtureAppPaths, FixtureDatabase,
    FixtureSecretStore, RuntimeAiConfig, RuntimeAiRequest,
};

fn saved_endpoint_and_key(account_id: &str) -> (dh_bot_lib::models::AiProviderEndpoint, String) {
    let paths = FixtureAppPaths::discover().expect("DH paths should resolve");
    assert_eq!(paths.runtime_mode(), "real");
    let database = FixtureDatabase::open(&paths).expect("DH database should open");
    let endpoint = database
        .list_ai_provider_endpoints(account_id)
        .expect("AI endpoints should load")
        .into_iter()
        .filter(|endpoint| endpoint.enabled)
        .min_by_key(|endpoint| (endpoint.priority, endpoint.id))
        .expect("an enabled AI endpoint is required");
    let secrets = FixtureSecretStore::new(&paths.secrets)
        .load()
        .expect("saved secrets should load");
    let key = secrets
        .get(&endpoint.secret_ref)
        .cloned()
        .expect("the configured AI key is required");
    (endpoint, key)
}

#[test]
#[ignore = "requires explicit confirmation before changing the real knowledge database"]
fn manages_temporary_real_group_knowledge() {
    assert_eq!(
        std::env::var("DH_REAL_KNOWLEDGE_CONFIRM").as_deref(),
        Ok("MANAGE_TEMPORARY_KNOWLEDGE"),
        "DH_REAL_KNOWLEDGE_CONFIRM must be MANAGE_TEMPORARY_KNOWLEDGE"
    );
    let account_id =
        std::env::var("DH_REAL_AI_ACCOUNT_ID").expect("DH_REAL_AI_ACCOUNT_ID is required");
    let group_id = std::env::var("DH_REAL_KNOWLEDGE_GROUP_ID")
        .expect("DH_REAL_KNOWLEDGE_GROUP_ID is required")
        .parse::<i64>()
        .expect("DH_REAL_KNOWLEDGE_GROUP_ID must be an integer");
    let action = std::env::var("DH_REAL_KNOWLEDGE_ACTION")
        .unwrap_or_else(|_| "bind".into())
        .to_ascii_lowercase();
    let name = "DH 真实知识测试 0731";
    let paths = FixtureAppPaths::discover().expect("DH paths should resolve");
    assert_eq!(paths.runtime_mode(), "real");
    let database = FixtureDatabase::open(&paths).expect("DH database should open");
    let existing = database
        .list_knowledge_bases(&account_id)
        .expect("knowledge bases should load")
        .into_iter()
        .find(|base| base.name == name);

    if action == "unbind" {
        if let Some(base) = existing {
            database
                .bind_knowledge_base(base.id, &account_id, &[])
                .expect("temporary knowledge binding should be removed");
            database
                .delete_knowledge_base(&account_id, base.id)
                .expect("temporary knowledge base should be deleted");
        }
        assert!(database
            .list_knowledge_bases(&account_id)
            .expect("knowledge bases should reload")
            .into_iter()
            .all(|base| base.name != name));
        eprintln!("temporary real knowledge removed: group_id={group_id}");
        return;
    }
    assert_eq!(
        action, "bind",
        "DH_REAL_KNOWLEDGE_ACTION must be bind or unbind"
    );

    let base_id = match existing {
        Some(base) => base.id,
        None => database
            .create_knowledge_base(&KnowledgeBase {
                id: 0,
                account_id: account_id.clone(),
                name: name.into(),
                description: "真实群绑定与解除验收，完成后自动删除".into(),
                enabled: true,
                built_in: false,
                read_only: false,
            })
            .expect("temporary knowledge base should be created"),
    };
    let content = "真实测试口令 KB0731-ALPHA 对应答案是海风七号。";
    let document_id = database
        .upsert_knowledge_document(&KnowledgeDocument {
            id: 0,
            base_id,
            base_name: name.into(),
            title: "真实绑定测试口令".into(),
            kind: "faq".into(),
            content: content.into(),
            source: "real-test-20260731".into(),
            content_hash: "dh-real-knowledge-kb0731-alpha-v1".into(),
            enabled: true,
        })
        .expect("temporary knowledge document should be saved");
    database
        .replace_knowledge_chunks(
            document_id,
            &[KnowledgeChunk {
                id: 0,
                document_id,
                chunk_index: 0,
                content: content.into(),
                content_hash: "dh-real-knowledge-kb0731-alpha-chunk-v1".into(),
                token_count: content.chars().count() as i64,
                enabled: true,
            }],
        )
        .expect("temporary knowledge chunks should be saved");
    database
        .bind_knowledge_base(base_id, &account_id, &[group_id])
        .expect("temporary knowledge base should bind to the test group");
    assert!(database
        .list_knowledge_for_group(&account_id, group_id)
        .expect("group knowledge should load")
        .into_iter()
        .any(|document| document.id == document_id && document.content == content));
    eprintln!(
        "temporary real knowledge bound: base_id={base_id}, document_id={document_id}, group_id={group_id}"
    );
}

#[test]
#[ignore = "requires explicit local AI configuration variables"]
fn configures_saved_ai_provider_from_environment() {
    assert_eq!(
        std::env::var("DH_REAL_AI_CONFIGURE_CONFIRM").as_deref(),
        Ok("CONFIGURE_SAVED_PROVIDER"),
        "DH_REAL_AI_CONFIGURE_CONFIRM must be CONFIGURE_SAVED_PROVIDER"
    );
    let account_id =
        std::env::var("DH_REAL_AI_ACCOUNT_ID").expect("DH_REAL_AI_ACCOUNT_ID is required");
    let name = std::env::var("DH_REAL_AI_NAME").expect("DH_REAL_AI_NAME is required");
    let base_url = std::env::var("DH_REAL_AI_BASE_URL").expect("DH_REAL_AI_BASE_URL is required");
    let model = std::env::var("DH_REAL_AI_MODEL").expect("DH_REAL_AI_MODEL is required");
    let api_backend = std::env::var("DH_REAL_AI_BACKEND").expect("DH_REAL_AI_BACKEND is required");
    let api_key = std::env::var("DH_REAL_AI_API_KEY").expect("DH_REAL_AI_API_KEY is required");
    assert!(base_url.starts_with("https://"));
    assert!(!api_key.trim().is_empty());

    let paths = FixtureAppPaths::discover().expect("DH paths should resolve");
    assert_eq!(paths.runtime_mode(), "real");
    let database = FixtureDatabase::open(&paths).expect("DH database should open");
    let mut endpoint = database
        .list_ai_provider_endpoints(&account_id)
        .expect("AI endpoints should load")
        .into_iter()
        .min_by_key(|endpoint| (endpoint.priority, endpoint.id))
        .expect("an AI endpoint is required");
    endpoint.name = name.clone();
    endpoint.base_url = base_url.clone();
    endpoint.webhook_url.clear();
    endpoint.api_backend = api_backend.clone();
    endpoint.model = model.clone();
    endpoint.enabled = true;
    endpoint.health_status = "unchecked".into();
    endpoint.failure_count = 0;
    endpoint.cooldown_until = None;
    endpoint.last_error.clear();
    database
        .save_ai_provider_endpoint(&endpoint)
        .expect("AI endpoint should save");
    database
        .set_setting("ai.base_url", &base_url, false)
        .expect("legacy Base URL should stay aligned");
    database
        .set_setting("ai.model", &model, false)
        .expect("legacy model should stay aligned");
    database
        .set_setting("ai.api_backend", &api_backend, false)
        .expect("legacy backend should stay aligned");
    let secrets = FixtureSecretStore::new(&paths.secrets);
    let mut values = secrets.load().expect("saved secrets should load");
    values.insert(endpoint.secret_ref.clone(), api_key);
    secrets.save(&values).expect("AI secret should save");

    let saved = database
        .list_ai_provider_endpoints(&account_id)
        .expect("saved AI endpoint should reload")
        .into_iter()
        .find(|value| value.id == endpoint.id)
        .expect("saved endpoint should exist");
    assert_eq!(saved.name, name);
    assert_eq!(saved.base_url, base_url);
    assert_eq!(saved.model, model);
    assert_eq!(saved.api_backend, api_backend);
    assert!(saved.enabled);
    assert!(secrets
        .load()
        .expect("saved secret should reload")
        .get(&saved.secret_ref)
        .is_some_and(|value| !value.trim().is_empty()));
    eprintln!(
        "saved AI provider configured: name={}, model={}, backend={}",
        saved.name, saved.model, saved.api_backend
    );
}

#[tokio::test]
#[ignore = "requires the local DH AI configuration and explicit confirmation"]
async fn tests_saved_ai_provider_without_wangshangliao_or_group_writes() {
    assert_eq!(
        std::env::var("DH_REAL_AI_CONFIRM").as_deref(),
        Ok("TEST_SAVED_PROVIDER"),
        "DH_REAL_AI_CONFIRM must be TEST_SAVED_PROVIDER"
    );
    let account_id =
        std::env::var("DH_REAL_AI_ACCOUNT_ID").expect("DH_REAL_AI_ACCOUNT_ID is required");
    let paths = FixtureAppPaths::discover().expect("DH paths should resolve");
    assert_eq!(paths.runtime_mode(), "real");
    let database = FixtureDatabase::open(&paths).expect("DH database should open");
    let endpoint = database
        .list_ai_provider_endpoints(&account_id)
        .expect("AI endpoints should load")
        .into_iter()
        .filter(|endpoint| endpoint.enabled)
        .min_by_key(|endpoint| (endpoint.priority, endpoint.id))
        .expect("an enabled AI endpoint is required");
    let secrets = FixtureSecretStore::new(&paths.secrets)
        .load()
        .expect("saved secrets should load");
    let api_key = secrets
        .get(&endpoint.secret_ref)
        .cloned()
        .expect("the configured AI key is required");
    assert!(!api_key.trim().is_empty());
    let provider = ConfiguredAiProviderFactory::default()
        .create(vec![RuntimeAiConfig {
            base_url: endpoint.base_url.clone(),
            webhook_url: endpoint.webhook_url.clone(),
            api_backend: std::env::var("DH_REAL_AI_OVERRIDE_BACKEND")
                .unwrap_or_else(|_| endpoint.api_backend.clone()),
            model: std::env::var("DH_REAL_AI_OVERRIDE_MODEL")
                .unwrap_or_else(|_| endpoint.model.clone()),
            reasoning_effort: std::env::var("DH_REAL_AI_OVERRIDE_REASONING")
                .unwrap_or_else(|_| endpoint.reasoning_effort.clone()),
            api_key,
            timeout: Duration::from_secs(60),
        }])
        .expect("AI provider should initialize");
    let prompt = std::env::var("DH_REAL_AI_PROMPT")
        .unwrap_or_else(|_| "请用一句自然中文回答：DH BOT 的 AI 连接测试正常吗？".into());
    let started = Instant::now();
    let decision = provider
        .decide(&RuntimeAiRequest::testing(prompt, Vec::new()))
        .await
        .expect("saved AI provider should reply");
    eprintln!("saved AI provider decision: {:?}", decision);
    assert!(!decision.reply.trim().is_empty());
    assert!(decision.actions.is_empty());
    assert!(decision.tasks.is_empty());
    database
        .update_ai_provider_health(&account_id, endpoint.id, true, "")
        .expect("successful real provider verification should update endpoint health");
    eprintln!(
        "saved AI provider passed: model={}, elapsed_ms={}, reply_chars={}",
        std::env::var("DH_REAL_AI_OVERRIDE_MODEL").unwrap_or(endpoint.model),
        started.elapsed().as_millis(),
        decision.reply.chars().count()
    );
}

#[tokio::test]
#[ignore = "requires the local DH AI configuration and explicit confirmation"]
async fn probes_saved_ai_model_catalog_without_group_writes() {
    assert_eq!(
        std::env::var("DH_REAL_AI_MODELS_CONFIRM").as_deref(),
        Ok("PROBE_SAVED_MODELS"),
        "DH_REAL_AI_MODELS_CONFIRM must be PROBE_SAVED_MODELS"
    );
    let account_id =
        std::env::var("DH_REAL_AI_ACCOUNT_ID").expect("DH_REAL_AI_ACCOUNT_ID is required");
    let (endpoint, api_key) = saved_endpoint_and_key(&account_id);
    let models_url = format!("{}/models", endpoint.base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(10))
        .build()
        .expect("AI probe client should initialize");
    let response = client
        .get(models_url)
        .bearer_auth(api_key)
        .send()
        .await
        .expect("saved AI model catalog should respond");
    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .expect("saved AI model catalog should be JSON");
    assert!(status.is_success(), "model catalog returned HTTP {status}");
    let models = body
        .get("data")
        .and_then(serde_json::Value::as_array)
        .expect("model catalog should contain data");
    let model_ids = models
        .iter()
        .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    eprintln!("available model ids: {}", model_ids.join(", "));
    let expected_model =
        std::env::var("DH_REAL_AI_EXPECT_MODEL").unwrap_or_else(|_| endpoint.model.clone());
    assert!(
        models.iter().any(|model| {
            model
                .get("id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| id == expected_model || id.starts_with(&expected_model))
        }),
        "expected model should be listed"
    );
    eprintln!(
        "saved AI model catalog passed: expected_model={}, listed={}",
        expected_model,
        models.len()
    );
}

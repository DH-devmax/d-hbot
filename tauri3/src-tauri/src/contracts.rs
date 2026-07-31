use serde::Deserialize;

use crate::gateway::{CapabilitySource, CapabilityStatus, GatewayCapabilities, GatewayCapability};

const PRODUCTION_CAPABILITIES: &str =
    include_str!("../../contracts/wangshangliao_capabilities.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProductionCalibration {
    app_file_version: String,
    main_script_sha256: String,
    verified: bool,
    capabilities: GatewayCapabilities,
}

#[derive(Debug, Deserialize)]
struct ProductionCapabilityRegistry {
    version: u8,
    calibrations: Vec<ProductionCalibration>,
}

pub(crate) fn production_capabilities(
    app_file_version: &str,
    main_script_sha256: &str,
) -> GatewayCapabilities {
    capabilities_from_registry(
        PRODUCTION_CAPABILITIES,
        app_file_version,
        main_script_sha256,
    )
}

pub(crate) fn runtime_capabilities(
    app_file_version: &str,
    main_script_sha256: &str,
) -> GatewayCapabilities {
    #[cfg(feature = "fixture")]
    {
        let engine = ContractReplayEngine::frozen();
        let metadata = engine.metadata();
        if metadata.app_file_version == app_file_version
            && metadata
                .main_script_sha256
                .eq_ignore_ascii_case(main_script_sha256)
        {
            return engine.capabilities_for(app_file_version, main_script_sha256);
        }
    }
    production_capabilities(app_file_version, main_script_sha256)
}

#[cfg(feature = "fixture")]
pub(crate) fn fixture_calibration_identity() -> (String, String) {
    let engine = ContractReplayEngine::frozen();
    let metadata = engine.metadata();
    (
        metadata.app_file_version.clone(),
        metadata.main_script_sha256.clone(),
    )
}

fn capabilities_from_registry(
    raw: &str,
    app_file_version: &str,
    main_script_sha256: &str,
) -> GatewayCapabilities {
    let Ok(registry) = serde_json::from_str::<ProductionCapabilityRegistry>(raw) else {
        return unverified_production_capabilities();
    };
    if registry.version != 1 {
        return unverified_production_capabilities();
    }
    registry
        .calibrations
        .into_iter()
        .find(|entry| {
            entry.verified
                && entry.app_file_version == app_file_version
                && entry
                    .main_script_sha256
                    .eq_ignore_ascii_case(main_script_sha256)
        })
        .map(|entry| entry.capabilities)
        .unwrap_or_else(unverified_production_capabilities)
}

pub(crate) fn unverified_production_capabilities() -> GatewayCapabilities {
    let pending = |reason: &str| {
        GatewayCapability::manual_verification(
            CapabilitySource::WangElectron,
            reason,
            String::new(),
        )
    };
    GatewayCapabilities {
        announcement: pending("等待旺商聊公告协议只读探测"),
        send_text: pending("等待消息编码与 NIM 方法探测"),
        mute: pending("等待 ZCG 路由基线探测"),
        recall: pending("等待 ZCG 路由基线探测"),
        rename: pending("等待 ZCG 路由基线探测"),
        remove_member: GatewayCapability::new(
            CapabilityStatus::ManualVerification,
            CapabilitySource::ZcgContract,
            true,
            false,
            "等待 ZCG 移出成员路由基线探测",
            String::new(),
        ),
        group_mute: pending("等待 ZCG 路由基线探测"),
        member_events: pending("等待 NIM 成员事件监听探测"),
    }
}

#[cfg(any(feature = "fixture", test))]
mod gateway_v2 {
    use std::collections::BTreeMap;

    use serde::Deserialize;
    use serde_json::Value;

    use crate::error::{AppError, AppResult};
    use crate::gateway::{CapabilityStatus, GatewayCapabilities, GatewayReceipt};

    const CONTRACT_RAW: &str = include_str!("../../contracts/group_gateway_v2.json");

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub(crate) struct GatewayContractMetadata {
        pub app_file_version: String,
        pub page_title: String,
        pub page_url: String,
        pub main_script_sha256: String,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GatewayCalibration {
        app_file_version: String,
        main_script_sha256: String,
        verified: bool,
        capabilities: GatewayCapabilities,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ContractRequest {
        params: Value,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ContractOperation {
        name: String,
        route: String,
        request: ContractRequest,
        transport: Value,
        business: Value,
        normalized_receipt: GatewayReceipt,
        expected_normalized_state: Value,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GatewayContractV2 {
        version: u8,
        metadata: GatewayContractMetadata,
        calibrations: Vec<GatewayCalibration>,
        operations: Vec<ContractOperation>,
        callbacks: Vec<Value>,
        expected_normalized_state: Value,
    }

    #[derive(Debug, Clone)]
    pub(crate) struct ContractReplayEngine {
        contract: GatewayContractV2,
        operations_by_route: BTreeMap<String, ContractOperation>,
    }

    impl ContractReplayEngine {
        pub(crate) fn frozen() -> Self {
            let contract: GatewayContractV2 = serde_json::from_str(CONTRACT_RAW)
                .expect("group gateway Contract v2 must be valid JSON");
            Self::from_contract(contract)
                .expect("group gateway Contract v2 must be internally consistent")
        }

        fn from_contract(contract: GatewayContractV2) -> AppResult<Self> {
            if contract.version != 2 {
                return Err(AppError::new(
                    "fixture_contract_version",
                    format!("不支持的 Contract v2 版本：{}", contract.version),
                ));
            }
            if contract.callbacks.is_empty() {
                return Err(AppError::new(
                    "fixture_contract_callbacks",
                    "Contract v2 缺少回调轨迹",
                ));
            }
            require_object(
                &contract.expected_normalized_state,
                "expectedNormalizedState",
            )?;
            let mut operations_by_route = BTreeMap::new();
            for operation in &contract.operations {
                validate_operation_definition(operation)?;
                if operations_by_route
                    .insert(operation.route.clone(), operation.clone())
                    .is_some()
                {
                    return Err(AppError::new(
                        "fixture_contract_route",
                        format!("Contract v2 路由重复：{}", operation.route),
                    ));
                }
            }
            if operations_by_route.is_empty() {
                return Err(AppError::new(
                    "fixture_contract_operations",
                    "Contract v2 缺少操作轨迹",
                ));
            }
            Ok(Self {
                contract,
                operations_by_route,
            })
        }

        pub(crate) fn metadata(&self) -> &GatewayContractMetadata {
            &self.contract.metadata
        }

        pub(crate) fn capabilities_for(
            &self,
            app_file_version: &str,
            main_script_sha256: &str,
        ) -> GatewayCapabilities {
            self.contract
                .calibrations
                .iter()
                .find(|entry| {
                    entry.verified
                        && entry.app_file_version == app_file_version
                        && entry
                            .main_script_sha256
                            .eq_ignore_ascii_case(main_script_sha256)
                })
                .map(|entry| entry.capabilities.clone())
                .unwrap_or_else(unverified_capabilities)
        }

        pub(crate) fn validate_request(&self, route: &str, params: &Value) -> AppResult<String> {
            let operation = self.operations_by_route.get(route).ok_or_else(|| {
                AppError::new(
                    "fixture_contract_route",
                    format!("Contract v2 未冻结路由：{route}"),
                )
            })?;
            validate_shape(&operation.request.params, params, "params")?;
            Ok(operation.name.clone())
        }

        #[cfg(test)]
        /// Validates one replayed operation against every frozen Contract v2
        /// layer. This intentionally compares raw envelopes and normalized
        /// output separately: a protocol adapter may normalize wording, but it
        /// must not silently change codes, identifiers, receipt fields, or the
        /// resulting state.
        pub(crate) fn validate_operation_result(
            &self,
            route: &str,
            transport: &Value,
            business: &Value,
            receipt: &GatewayReceipt,
            normalized_state: &Value,
        ) -> AppResult<()> {
            let operation = self.operations_by_route.get(route).ok_or_else(|| {
                AppError::new(
                    "fixture_contract_route",
                    format!("Contract v2 未冻结动作：{route}"),
                )
            })?;
            validate_exact(&operation.transport, transport, "transport")?;
            validate_exact(&operation.business, business, "business")?;
            if &operation.normalized_receipt != receipt {
                return Err(AppError::new(
                    "fixture_contract_receipt",
                    format!("{} 的标准化回执与 Contract v2 不一致", operation.name),
                ));
            }
            validate_exact(
                &operation.expected_normalized_state,
                normalized_state,
                "expectedNormalizedState",
            )?;
            Ok(())
        }

        pub(crate) fn success_receipt(
            &self,
            route: &str,
            ordinal: usize,
            message_id: String,
        ) -> AppResult<GatewayReceipt> {
            let operation = self.operations_by_route.get(route).ok_or_else(|| {
                AppError::new(
                    "fixture_contract_route",
                    format!("Contract v2 未冻结动作：{route}"),
                )
            })?;
            let mut receipt = operation.normalized_receipt.clone();
            receipt.request_id = format!("fixture-request-{ordinal}");
            if !message_id.is_empty() {
                receipt.message_id = message_id;
            }
            Ok(receipt)
        }
    }

    fn validate_operation_definition(operation: &ContractOperation) -> AppResult<()> {
        if operation.name.trim().is_empty() || operation.route.trim().is_empty() {
            return Err(AppError::new(
                "fixture_contract_operation",
                "Contract v2 操作缺少名称或路由",
            ));
        }
        require_object(
            &operation.request.params,
            &format!("{}.request.params", operation.name),
        )?;
        require_envelope_fields(
            &operation.transport,
            &format!("{}.transport", operation.name),
            &["transportCode", "errno", "requestId"],
        )?;
        require_envelope_fields(
            &operation.business,
            &format!("{}.business", operation.name),
            &["code", "errno", "msg", "data"],
        )?;
        require_object(
            &operation.expected_normalized_state,
            &format!("{}.expectedNormalizedState", operation.name),
        )?;
        if operation.normalized_receipt.route != operation.route
            || operation.normalized_receipt.status != "succeeded"
        {
            return Err(AppError::new(
                "fixture_contract_receipt",
                format!("{} 的标准化回执缺少成功状态或路由", operation.name),
            ));
        }
        assert_receipt_code(
            &operation.normalized_receipt.transport_code,
            &operation.transport,
            "transportCode",
            &operation.name,
        )?;
        assert_receipt_code(
            &operation.normalized_receipt.transport_errno,
            &operation.transport,
            "errno",
            &operation.name,
        )?;
        assert_receipt_code(
            &operation.normalized_receipt.business_code,
            &operation.business,
            "code",
            &operation.name,
        )?;
        assert_receipt_code(
            &operation.normalized_receipt.business_errno,
            &operation.business,
            "errno",
            &operation.name,
        )?;
        if !operation.normalized_receipt.request_id.is_empty()
            && operation.normalized_receipt.request_id
                != operation.transport["requestId"]
                    .as_str()
                    .unwrap_or_default()
        {
            return Err(AppError::new(
                "fixture_contract_receipt",
                format!("{} 的 requestId 与 transport 不一致", operation.name),
            ));
        }
        Ok(())
    }

    fn require_object(value: &Value, path: &str) -> AppResult<()> {
        if value.is_object() {
            Ok(())
        } else {
            Err(AppError::new(
                "fixture_contract_shape",
                format!("Contract v2 字段必须是对象：{path}"),
            ))
        }
    }

    fn require_envelope_fields(value: &Value, path: &str, fields: &[&str]) -> AppResult<()> {
        require_object(value, path)?;
        for field in fields {
            if value.get(*field).is_none() {
                return Err(AppError::new(
                    "fixture_contract_envelope",
                    format!("Contract v2 缺少 {path}.{field}"),
                ));
            }
        }
        if !value[fields[0]].is_number() || !value[fields[1]].is_number() {
            return Err(AppError::new(
                "fixture_contract_envelope",
                format!("Contract v2 状态码类型错误：{path}"),
            ));
        }
        if !value[fields[2]].is_string() {
            return Err(AppError::new(
                "fixture_contract_envelope",
                format!("Contract v2 标识或消息类型错误：{path}.{}", fields[2]),
            ));
        }
        Ok(())
    }

    fn assert_receipt_code(
        receipt: &Option<i64>,
        envelope: &Value,
        field: &str,
        operation: &str,
    ) -> AppResult<()> {
        let expected = envelope[field].as_i64();
        if *receipt != expected {
            return Err(AppError::new(
                "fixture_contract_receipt",
                format!("{operation} 的回执 {field} 与原始 envelope 不一致"),
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    fn validate_exact(expected: &Value, actual: &Value, path: &str) -> AppResult<()> {
        if expected == actual {
            return Ok(());
        }
        Err(AppError::new(
            "fixture_contract_result",
            format!("Contract v2 实际 {path} 与冻结轨迹逐字段不一致"),
        ))
    }

    fn unverified_capabilities() -> GatewayCapabilities {
        GatewayCapabilities {
            announcement: CapabilityStatus::Unverified.into(),
            send_text: CapabilityStatus::Unverified.into(),
            mute: CapabilityStatus::Unverified.into(),
            recall: CapabilityStatus::Unverified.into(),
            rename: CapabilityStatus::Unverified.into(),
            remove_member: CapabilityStatus::Unverified.into(),
            group_mute: CapabilityStatus::Unverified.into(),
            member_events: CapabilityStatus::Unverified.into(),
        }
    }

    fn validate_shape(expected: &Value, actual: &Value, path: &str) -> AppResult<()> {
        match (expected, actual) {
            (Value::Object(expected), Value::Object(actual)) => {
                for (key, expected_value) in expected {
                    let actual_value = actual.get(key).ok_or_else(|| {
                        AppError::new(
                            "fixture_contract_params",
                            format!("Contract v2 请求缺少 {path}.{key}"),
                        )
                    })?;
                    validate_shape(expected_value, actual_value, &format!("{path}.{key}"))?;
                }
                Ok(())
            }
            (Value::Array(expected), Value::Array(actual)) => {
                if let Some(expected_item) = expected.first() {
                    for (index, actual_item) in actual.iter().enumerate() {
                        validate_shape(expected_item, actual_item, &format!("{path}[{index}]"))?;
                    }
                }
                Ok(())
            }
            (Value::String(_), Value::String(_))
            | (Value::Number(_), Value::Number(_))
            | (Value::Bool(_), Value::Bool(_))
            | (Value::Null, Value::Null) => Ok(()),
            _ => Err(AppError::new(
                "fixture_contract_params",
                format!("Contract v2 请求字段类型不匹配：{path}"),
            )),
        }
    }
}

#[cfg(any(feature = "fixture", test))]
pub(crate) use gateway_v2::ContractReplayEngine;

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{capabilities_from_registry, production_capabilities, ContractReplayEngine};
    use crate::gateway::{CapabilityStatus, GatewayCapabilities};

    fn fixture(raw: &str) -> Value {
        serde_json::from_str(raw).expect("contract fixture must be valid JSON")
    }

    #[test]
    fn frozen_contracts_have_explicit_versions() {
        let values = [
            fixture(include_str!("../../contracts/group_gateway_v1.json")),
            fixture(include_str!("../../contracts/group_gateway_v2.json")),
            fixture(include_str!("../../contracts/ai_provider_v1.json")),
            fixture(include_str!("../../contracts/prediction_v1.json")),
            fixture(include_str!("../../contracts/schedule_v1.json")),
            fixture(include_str!(
                "../../contracts/wangshangliao_capabilities.json"
            )),
        ];
        assert!(values.iter().all(|value| value.get("version").is_some()));
    }

    #[test]
    fn gateway_v2_freezes_metadata_envelopes_callbacks_and_state() {
        let value = fixture(include_str!("../../contracts/group_gateway_v2.json"));
        let metadata = &value["metadata"];
        assert!(metadata["appFileVersion"].is_string());
        assert!(metadata["pageTitle"].is_string());
        assert!(metadata["pageUrl"].is_string());
        assert_eq!(metadata["mainScriptSha256"].as_str().unwrap().len(), 64);
        assert!(value["callbacks"].as_array().unwrap().len() >= 4);
        assert!(value["expectedNormalizedState"].is_object());
        assert_eq!(
            value["businessFailures"]
                .as_array()
                .unwrap()
                .iter()
                .find(|failure| failure["name"] == "nimNotReady")
                .unwrap()["transportCode"],
            503
        );
        assert!(value["operations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|operation| {
                operation["request"]["params"].is_object()
                    && operation["transport"].is_object()
                    && operation["business"].is_object()
                    && operation["normalizedReceipt"].is_object()
                    && operation["expectedNormalizedState"].is_object()
            }));

        let session = value["batch"]["session"].as_str().unwrap();
        let records = value["batch"]["records"].as_array().unwrap();
        assert!(records.len() <= value["limits"]["batch"].as_u64().unwrap() as usize);
        assert!(records
            .iter()
            .all(|record| record["session"].as_str() == Some(session)));
        assert!(records
            .windows(2)
            .all(|pair| pair[0]["sequence"].as_u64().unwrap()
                < pair[1]["sequence"].as_u64().unwrap()));
    }

    #[test]
    fn archived_go_requests_match_gateway_v2_field_for_field() {
        let v1 = fixture(include_str!("../../contracts/group_gateway_v1.json"));
        let v2 = fixture(include_str!("../../contracts/group_gateway_v2.json"));
        for legacy in v1["operations"].as_array().unwrap() {
            let route = legacy["route"].as_str().unwrap();
            let current = v2["operations"]
                .as_array()
                .unwrap()
                .iter()
                .find(|operation| operation["route"].as_str() == Some(route))
                .unwrap_or_else(|| panic!("Contract v2 缺少归档 Go 路由：{route}"));
            assert_eq!(
                legacy["request"], current["request"]["params"],
                "归档 Go 与 Rust Contract v2 请求不一致：{route}"
            );
        }
    }

    #[test]
    fn calibration_is_keyed_by_file_version_and_script_hash() {
        let engine = ContractReplayEngine::frozen();
        let metadata = engine.metadata();
        let known =
            engine.capabilities_for(&metadata.app_file_version, &metadata.main_script_sha256);
        assert_eq!(known.rename, CapabilityStatus::Supported);
        assert_eq!(known.send_text, CapabilityStatus::Supported);
        assert_eq!(known.announcement, CapabilityStatus::Supported);

        let unknown_hash = engine.capabilities_for(&metadata.app_file_version, &"f".repeat(64));
        assert_eq!(unknown_hash.rename, CapabilityStatus::Unverified);
        assert_eq!(unknown_hash.send_text, CapabilityStatus::Unverified);
        assert_eq!(unknown_hash.mute, CapabilityStatus::Unverified);
        assert_eq!(unknown_hash.announcement, CapabilityStatus::Unverified);
    }

    #[test]
    fn production_registry_defaults_unknown_versions_to_read_only() {
        let unknown = production_capabilities("unknown", &"f".repeat(64));
        assert_eq!(unknown.announcement, CapabilityStatus::Unverified);
        assert_eq!(unknown.send_text, CapabilityStatus::Unverified);
        assert_eq!(unknown.rename, CapabilityStatus::Unverified);
        assert_eq!(unknown.remove_member, CapabilityStatus::Unverified);

        let expected = GatewayCapabilities {
            announcement: CapabilityStatus::Unsupported.into(),
            send_text: CapabilityStatus::Supported.into(),
            mute: CapabilityStatus::Supported.into(),
            recall: CapabilityStatus::Supported.into(),
            rename: CapabilityStatus::Supported.into(),
            remove_member: CapabilityStatus::Supported.into(),
            group_mute: CapabilityStatus::Supported.into(),
            member_events: CapabilityStatus::Supported.into(),
        };
        let raw = serde_json::json!({
            "version": 1,
            "calibrations": [{
                "appFileVersion": "2.6.3",
                "mainScriptSha256": "a".repeat(64),
                "verified": true,
                "capabilities": expected,
            }]
        })
        .to_string();
        let restored = capabilities_from_registry(&raw, "2.6.3", &"a".repeat(64));
        assert_eq!(restored.announcement, CapabilityStatus::Unsupported);
        assert_eq!(restored.send_text, CapabilityStatus::Supported);
        assert_eq!(restored.remove_member, CapabilityStatus::Supported);
    }

    #[test]
    fn verified_277_build_enables_only_captured_announcement_write() {
        for hash in [
            "17af5c0697c9a091f3112a26d21a7815dd1d635a7e424c392986dc53ce5fd149",
            "52da37f6d0ea6419d58caee55f4ae1aac083e4d8fa641f46529e2e4b91e627e2",
        ] {
            let capabilities = production_capabilities("2.7.7", hash);
            assert_eq!(capabilities.announcement, CapabilityStatus::Supported);
            assert_eq!(capabilities.send_text, CapabilityStatus::Unverified);
            assert_eq!(capabilities.rename, CapabilityStatus::Unverified);
            assert_eq!(capabilities.remove_member, CapabilityStatus::Unsupported);
        }
    }

    #[test]
    fn replay_engine_validates_frozen_request_shapes() {
        let engine = ContractReplayEngine::frozen();
        assert_eq!(
            engine
                .validate_request(
                    "/v1/group/set-member-mute",
                    &json!({"groupId":1143980,"userId":10006,"min":10}),
                )
                .unwrap(),
            "mute"
        );
        assert!(engine
            .validate_request(
                "/v1/group/set-member-mute",
                &json!({"groupId":1143980,"userId":"10006","min":10}),
            )
            .is_err());
    }

    #[test]
    fn replay_engine_validates_transport_business_receipt_and_state_exactly() {
        let engine = ContractReplayEngine::frozen();
        let contract = fixture(include_str!("../../contracts/group_gateway_v2.json"));
        for operation in contract["operations"].as_array().unwrap() {
            let route = operation["route"].as_str().unwrap();
            let receipt = serde_json::from_value(operation["normalizedReceipt"].clone()).unwrap();
            engine
                .validate_operation_result(
                    route,
                    &operation["transport"],
                    &operation["business"],
                    &receipt,
                    &operation["expectedNormalizedState"],
                )
                .unwrap_or_else(|error| panic!("{route} should match frozen contract: {error}"));
        }
    }

    #[test]
    fn replay_engine_rejects_one_field_drift_in_each_contract_layer() {
        let engine = ContractReplayEngine::frozen();
        let contract = fixture(include_str!("../../contracts/group_gateway_v2.json"));
        let operation = contract["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|operation| operation["route"] == "/v1/group/set-member-mute")
            .unwrap();
        let route = operation["route"].as_str().unwrap();
        let receipt: crate::gateway::GatewayReceipt =
            serde_json::from_value(operation["normalizedReceipt"].clone()).unwrap();

        let mut transport = operation["transport"].clone();
        transport["transportCode"] = json!(201);
        assert!(engine
            .validate_operation_result(
                route,
                &transport,
                &operation["business"],
                &receipt,
                &operation["expectedNormalizedState"],
            )
            .is_err());

        let mut business = operation["business"].clone();
        business["errno"] = json!(7);
        assert!(engine
            .validate_operation_result(
                route,
                &operation["transport"],
                &business,
                &receipt,
                &operation["expectedNormalizedState"],
            )
            .is_err());

        let mut changed_receipt = receipt.clone();
        changed_receipt.business_errno = Some(7);
        assert!(engine
            .validate_operation_result(
                route,
                &operation["transport"],
                &operation["business"],
                &changed_receipt,
                &operation["expectedNormalizedState"],
            )
            .is_err());

        let mut state = operation["expectedNormalizedState"].clone();
        state["durationSeconds"] = json!(601);
        assert!(engine
            .validate_operation_result(
                route,
                &operation["transport"],
                &operation["business"],
                &receipt,
                &state,
            )
            .is_err());
    }
}

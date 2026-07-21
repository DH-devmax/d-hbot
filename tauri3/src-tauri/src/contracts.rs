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
            assert_eq!(contract.version, 2, "unsupported gateway contract version");
            assert!(
                !contract.callbacks.is_empty(),
                "gateway contract must freeze callbacks"
            );
            assert!(
                contract.expected_normalized_state.is_object(),
                "gateway contract must freeze expected normalized state"
            );
            let operations_by_route = contract
                .operations
                .iter()
                .cloned()
                .map(|operation| (operation.route.clone(), operation))
                .collect();
            Self {
                contract,
                operations_by_route,
            }
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
            if !operation.expected_normalized_state.is_object() {
                return Err(AppError::new(
                    "fixture_contract_state",
                    format!("{} 缺少标准化状态", operation.name),
                ));
            }
            let mut receipt = operation.normalized_receipt.clone();
            receipt.request_id = format!("fixture-request-{ordinal}");
            if !message_id.is_empty() {
                receipt.message_id = message_id;
            }
            Ok(receipt)
        }
    }

    fn unverified_capabilities() -> GatewayCapabilities {
        GatewayCapabilities {
            announcement: CapabilityStatus::Unverified,
            mute: CapabilityStatus::Unverified,
            recall: CapabilityStatus::Unverified,
            rename: CapabilityStatus::Unverified,
            remove_member: CapabilityStatus::Unverified,
            group_mute: CapabilityStatus::Unverified,
            member_events: CapabilityStatus::Unverified,
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

    use super::ContractReplayEngine;
    use crate::gateway::CapabilityStatus;

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
    fn calibration_is_keyed_by_file_version_and_script_hash() {
        let engine = ContractReplayEngine::frozen();
        let metadata = engine.metadata();
        let known =
            engine.capabilities_for(&metadata.app_file_version, &metadata.main_script_sha256);
        assert_eq!(known.rename, CapabilityStatus::Supported);
        assert_eq!(known.announcement, CapabilityStatus::Unsupported);

        let unknown_hash = engine.capabilities_for(&metadata.app_file_version, &"f".repeat(64));
        assert_eq!(unknown_hash.rename, CapabilityStatus::Unverified);
        assert_eq!(unknown_hash.mute, CapabilityStatus::Unverified);
        assert_eq!(unknown_hash.announcement, CapabilityStatus::Unverified);
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
}

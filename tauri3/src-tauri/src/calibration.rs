use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::gateway::{GatewayReceipt, GatewayRecord};

const WRITE_ROUTES: &[&str] = &[
    "nim.sendCustomMsg",
    "/v1/group/message-rollback",
    "/v1/group/set-member-mute",
    "/v1/group/member-mute-cancel",
    "/v1/group/set-member-nickname",
    "nim.updateNickInTeam",
    "/v1/group/remove-group-member",
    "/v1/group/set-group-mute",
    "/v1/group/add-notice",
    "/v1/group/notice-opt",
];

const ALLOWED_CAPABILITIES: &[&str] = &[
    "sendText",
    "recall",
    "mute",
    "rename",
    "announcement",
    "groupMute",
    "memberEvents",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationMetadata {
    pub app_file_version: String,
    pub main_script_sha256: String,
    pub page_title: String,
    pub page_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeveloperCalibrationStatus {
    pub active: bool,
    pub finishing: bool,
    pub started_at: String,
    pub app_file_version: String,
    pub main_script_sha256: String,
    pub page_title: String,
    pub page_url: String,
    pub capabilities: Vec<String>,
    pub operation_count: usize,
    pub callback_count: usize,
    pub write_operation_count: usize,
    pub baseline_count: usize,
    pub restored_baseline_count: usize,
    pub restoration_verified: bool,
    pub restoration_error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeveloperCalibrationExport {
    pub capture: Value,
    pub status: DeveloperCalibrationStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CalibrationOperation {
    name: String,
    route: String,
    request: Value,
    transport: Value,
    business: Value,
    normalized_receipt: GatewayReceipt,
    baseline_state: Option<Value>,
    expected_normalized_state: Value,
    captured_at: String,
}

#[derive(Debug, Clone)]
pub(crate) struct CalibrationRestorationTarget {
    pub route: String,
    pub identity: Value,
}

#[derive(Debug, Default)]
pub(crate) struct DeveloperCalibrationRecorder {
    metadata: Option<CalibrationMetadata>,
    finishing: bool,
    started_at: String,
    capabilities: BTreeSet<String>,
    operations: Vec<CalibrationOperation>,
    callbacks: Vec<Value>,
    baselines: BTreeMap<String, Value>,
    baseline_targets: BTreeMap<String, CalibrationRestorationTarget>,
    required_restorations: BTreeSet<String>,
    observed_states: BTreeMap<String, Value>,
}

impl DeveloperCalibrationRecorder {
    pub(crate) fn is_active(&self) -> bool {
        self.metadata.is_some()
    }

    pub(crate) fn is_finishing(&self) -> bool {
        self.finishing
    }

    pub(crate) fn start(
        &mut self,
        metadata: CalibrationMetadata,
        capabilities: Vec<String>,
    ) -> AppResult<DeveloperCalibrationStatus> {
        if self.metadata.is_some() {
            return Err(AppError::new(
                "calibration_active",
                "真实旺商聊校准采集已经在运行",
            ));
        }
        let normalized = normalize_capabilities(capabilities)?;
        self.metadata = Some(metadata);
        self.finishing = false;
        self.started_at = Utc::now().to_rfc3339();
        self.capabilities = normalized;
        self.operations.clear();
        self.callbacks.clear();
        self.baselines.clear();
        self.baseline_targets.clear();
        self.required_restorations.clear();
        self.observed_states.clear();
        Ok(self.status())
    }

    pub(crate) fn cancel(&mut self) -> AppResult<DeveloperCalibrationStatus> {
        if self.finishing {
            return Err(AppError::new(
                "calibration_finishing",
                "正在执行最终恢复回读和原子导出，暂时不能取消校准",
            ));
        }
        *self = Self::default();
        Ok(self.status())
    }

    pub(crate) fn status(&self) -> DeveloperCalibrationStatus {
        let metadata = self.metadata.clone().unwrap_or(CalibrationMetadata {
            app_file_version: String::new(),
            main_script_sha256: String::new(),
            page_title: String::new(),
            page_url: String::new(),
        });
        DeveloperCalibrationStatus {
            active: self.metadata.is_some(),
            finishing: self.finishing,
            started_at: self.started_at.clone(),
            app_file_version: metadata.app_file_version,
            main_script_sha256: metadata.main_script_sha256,
            page_title: metadata.page_title,
            page_url: metadata.page_url,
            capabilities: self.capabilities.iter().cloned().collect(),
            operation_count: self.operations.len(),
            callback_count: self.callbacks.len(),
            write_operation_count: self
                .operations
                .iter()
                .filter(|operation| is_write_route(&operation.route))
                .count(),
            baseline_count: self.baselines.len(),
            restored_baseline_count: self
                .baselines
                .iter()
                .filter(|(key, baseline)| self.observed_states.get(*key) == Some(*baseline))
                .count(),
            restoration_verified: self.restoration_report().0,
            restoration_error: self.restoration_report().1,
        }
    }

    pub(crate) fn allows(&self, capability: &str) -> bool {
        self.metadata.is_some() && !self.finishing && self.capabilities.contains(capability)
    }

    pub(crate) fn record_ipc(&mut self, route: &str, kind: &str, payload: Value, result: &Value) {
        if self.metadata.is_none() {
            return;
        }
        let transport_code = result
            .get("transportCode")
            .and_then(Value::as_i64)
            .unwrap_or(500);
        let transport_errno = result.get("errno").and_then(Value::as_i64).unwrap_or(1);
        let request_id = result
            .get("requestId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let business = business_envelope(result, transport_code, transport_errno);
        let business_code = business.get("code").and_then(Value::as_i64).unwrap_or(1);
        let business_errno = business.get("errno").and_then(Value::as_i64).unwrap_or(0);
        let business_message = business
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.operations.push(CalibrationOperation {
            name: operation_name(route, self.operations.len() + 1),
            route: route.to_string(),
            request: json!({"kind": kind, "params": payload}),
            transport: json!({
                "transportCode": transport_code,
                "errno": transport_errno,
                "requestId": request_id,
            }),
            business,
            normalized_receipt: GatewayReceipt {
                route: route.to_string(),
                status: if transport_code == 200
                    && transport_errno == 0
                    && business_code == 0
                    && business_errno == 0
                {
                    "succeeded"
                } else {
                    "failed"
                }
                .into(),
                transport_code: Some(transport_code),
                transport_errno: Some(transport_errno),
                business_code: Some(business_code),
                business_errno: Some(business_errno),
                business_message,
                request_id,
                message_id: String::new(),
                session: String::new(),
                acknowledged_through: 0,
                acknowledged: 0,
                remaining: 0,
                dropped: 0,
                verification: None,
            },
            baseline_state: None,
            expected_normalized_state: json!({"captured": true}),
            captured_at: Utc::now().to_rfc3339(),
        });
    }

    pub(crate) fn record_nim(&mut self, route: &str, payload: Value, result: &Value) {
        if self.metadata.is_none() {
            return;
        }
        let ok = result.get("ok").and_then(Value::as_bool) == Some(true);
        let code = if ok {
            0
        } else {
            result
                .get("errorCode")
                .or_else(|| result.get("code"))
                .and_then(Value::as_i64)
                .unwrap_or(1)
        };
        let request_id = result
            .get("requestId")
            .or_else(|| result.get("idClient"))
            .or_else(|| result.get("traceId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let message = result
            .get("message")
            .or_else(|| result.get("msg"))
            .or_else(|| result.get("errorMessage"))
            .and_then(Value::as_str)
            .unwrap_or(if ok { "OK" } else { "NIM 请求失败" })
            .to_string();
        let message_id = result
            .get("messageId")
            .or_else(|| result.get("idServer"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.operations.push(CalibrationOperation {
            name: operation_name(route, self.operations.len() + 1),
            route: route.to_string(),
            request: json!({"kind": "nim", "params": payload}),
            transport: json!({"transportCode": 200, "errno": 0, "requestId": request_id}),
            business: json!({"code": code, "errno": code, "msg": message, "data": result}),
            normalized_receipt: GatewayReceipt {
                route: route.to_string(),
                status: if ok { "succeeded" } else { "failed" }.into(),
                transport_code: Some(200),
                transport_errno: Some(0),
                business_code: Some(code),
                business_errno: Some(code),
                business_message: message,
                request_id,
                message_id,
                session: String::new(),
                acknowledged_through: 0,
                acknowledged: 0,
                remaining: 0,
                dropped: 0,
                verification: None,
            },
            baseline_state: None,
            expected_normalized_state: json!({"captured": true}),
            captured_at: Utc::now().to_rfc3339(),
        });
    }

    pub(crate) fn complete_operation(
        &mut self,
        route: &str,
        receipt: &GatewayReceipt,
        normalized_state: Value,
    ) {
        if self.metadata.is_none() {
            return;
        }
        if let Some(operation) = self
            .operations
            .iter_mut()
            .rfind(|operation| operation.route == route)
        {
            operation.normalized_receipt = receipt.clone();
            operation.expected_normalized_state = normalized_state;
        }
    }

    pub(crate) fn record_baseline(&mut self, route: &str, identity: Value, state: Value) {
        if self.metadata.is_none() || !requires_restoration(route) {
            return;
        }
        let key = state_key(route, &identity);
        self.required_restorations.insert(key.clone());
        self.baselines.entry(key.clone()).or_insert(state);
        self.baseline_targets
            .entry(key)
            .or_insert(CalibrationRestorationTarget {
                route: route.into(),
                identity,
            });
    }

    pub(crate) fn require_baseline(&mut self, route: &str, identity: Value) {
        if self.metadata.is_some() && requires_restoration(route) {
            let key = state_key(route, &identity);
            self.required_restorations.insert(key.clone());
            self.baseline_targets
                .entry(key)
                .or_insert(CalibrationRestorationTarget {
                    route: route.into(),
                    identity,
                });
        }
    }

    pub(crate) fn begin_finalization(&mut self) -> AppResult<()> {
        if self.metadata.is_none() {
            return Err(AppError::new(
                "calibration_inactive",
                "当前没有正在运行的校准采集",
            ));
        }
        if self.finishing {
            return Err(AppError::new(
                "calibration_finishing",
                "最终恢复回读和导出已经在进行中",
            ));
        }
        self.finishing = true;
        self.observed_states.clear();
        Ok(())
    }

    pub(crate) fn finalization_targets(&self) -> AppResult<Vec<CalibrationRestorationTarget>> {
        if !self.finishing {
            return Err(AppError::new(
                "calibration_not_finalizing",
                "校准尚未进入最终恢复回读状态",
            ));
        }
        Ok(self
            .required_restorations
            .iter()
            .filter_map(|key| self.baseline_targets.get(key).cloned())
            .collect())
    }

    pub(crate) fn abort_finalization(&mut self) {
        self.finishing = false;
    }

    pub(crate) fn record_restored_state(&mut self, route: &str, identity: Value, state: Value) {
        if self.metadata.is_none() || !requires_restoration(route) {
            return;
        }
        let key = state_key(route, &identity);
        self.observed_states.insert(key, state);
    }

    pub(crate) fn record_callbacks(&mut self, records: &[GatewayRecord]) {
        if self.metadata.is_none() {
            return;
        }
        self.callbacks.extend(
            records
                .iter()
                .filter_map(|record| serde_json::to_value(record).ok()),
        );
    }

    pub(crate) fn finish(&self, manually_restored: bool) -> AppResult<DeveloperCalibrationExport> {
        let Some(metadata) = self.metadata.clone() else {
            return Err(AppError::new(
                "calibration_inactive",
                "当前没有正在运行的校准采集",
            ));
        };
        if !self.finishing {
            return Err(AppError::new(
                "calibration_not_finalizing",
                "必须先冻结写操作并完成最终恢复回读",
            ));
        }
        if self.operations.is_empty() {
            return Err(AppError::new(
                "calibration_empty",
                "校准采集还没有记录任何旺商聊请求",
            ));
        }
        if self.callbacks.is_empty() {
            return Err(AppError::new(
                "calibration_callbacks",
                "校准采集还没有记录旺商聊回调，请等待一条测试群回调后再完成",
            ));
        }
        let write_operation_count = self
            .operations
            .iter()
            .filter(|operation| is_write_route(&operation.route))
            .count();
        let (restoration_verified, restoration_error) = self.restoration_report();
        if write_operation_count > 0 && !restoration_verified {
            return Err(AppError::new(
                "calibration_not_restored",
                format!("真实回读未确认所有状态已恢复：{restoration_error}"),
            ));
        }
        let status = self.status();
        let (baselines, observed_states) = self.restoration_evidence();
        let capture = json!({
            "version": 2,
            "metadata": {
                "captureFormat": "dh-contract-v2",
                "capturedAt": self.started_at,
                "appFileVersion": metadata.app_file_version,
                "pageTitle": metadata.page_title,
                "pageUrl": metadata.page_url,
                "mainScriptSha256": metadata.main_script_sha256,
            },
            "operations": self.operations,
            "callbacks": self.callbacks,
            "expectedNormalizedState": {
                "restored": restoration_verified,
                "restorationError": restoration_error,
                "manualRestoredAcknowledged": manually_restored,
                "baselines": baselines,
                "observedStates": observed_states,
                "operationCount": status.operation_count,
                "callbackCount": status.callback_count,
                "writeOperationCount": write_operation_count,
                "capabilities": status.capabilities,
            },
        });
        Ok(DeveloperCalibrationExport { capture, status })
    }

    pub(crate) fn commit(&mut self) {
        *self = Self::default();
    }

    fn restoration_evidence(&self) -> (Vec<Value>, Vec<Value>) {
        let evidence = |states: &BTreeMap<String, Value>| {
            states
                .iter()
                .filter_map(|(key, state)| {
                    let target = self.baseline_targets.get(key)?;
                    Some(json!({
                        "family": restoration_family(&target.route),
                        "identity": target.identity.clone(),
                        "state": state.clone(),
                    }))
                })
                .collect::<Vec<_>>()
        };
        (evidence(&self.baselines), evidence(&self.observed_states))
    }

    fn restoration_report(&self) -> (bool, String) {
        if self.required_restorations.is_empty() {
            return (true, String::new());
        }
        let mut missing = BTreeSet::new();
        let mut mismatched = BTreeSet::new();
        for key in &self.required_restorations {
            match (self.baselines.get(key), self.observed_states.get(key)) {
                (None, _) => {
                    missing.insert(key.clone());
                }
                (Some(_), None) => {
                    missing.insert(key.clone());
                }
                (Some(baseline), Some(observed)) if observed != baseline => {
                    mismatched.insert(key.clone());
                }
                (Some(_), Some(_)) => {}
            }
        }
        if missing.is_empty() && mismatched.is_empty() {
            (true, String::new())
        } else {
            let mut parts = Vec::new();
            if !missing.is_empty() {
                parts.push(format!(
                    "缺少基线或最终回读：{}",
                    missing.into_iter().collect::<Vec<_>>().join(", ")
                ));
            }
            if !mismatched.is_empty() {
                parts.push(format!(
                    "最终状态与基线不一致：{}",
                    mismatched.into_iter().collect::<Vec<_>>().join(", ")
                ));
            }
            (false, parts.join("；"))
        }
    }
}

fn normalize_capabilities(values: Vec<String>) -> AppResult<BTreeSet<String>> {
    let mut normalized = BTreeSet::new();
    for value in values {
        let Some(capability) = ALLOWED_CAPABILITIES
            .iter()
            .find(|capability| capability.eq_ignore_ascii_case(value.trim()))
        else {
            return Err(AppError::new(
                "calibration_capability",
                format!("不支持的校准能力：{value}"),
            ));
        };
        normalized.insert((*capability).to_string());
    }
    if normalized.is_empty() {
        return Err(AppError::new(
            "calibration_capability",
            "至少选择一项需要校准的能力",
        ));
    }
    Ok(normalized)
}

fn business_envelope(result: &Value, transport_code: i64, transport_errno: i64) -> Value {
    if let Some(response) = result.get("response").and_then(Value::as_str) {
        if let Ok(mut business) = serde_json::from_str::<Value>(response) {
            if let Some(object) = business.as_object_mut() {
                object.entry("code").or_insert(Value::from(0));
                object.entry("errno").or_insert(Value::from(0));
                object.entry("msg").or_insert(Value::from("OK"));
                object.entry("data").or_insert(Value::Null);
                return business;
            }
        }
        return json!({"code": 0, "errno": 0, "msg": "OK", "data": response});
    }
    let message = result
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("旺商聊传输请求失败");
    json!({
        "code": transport_code,
        "errno": transport_errno,
        "msg": message,
        "data": null,
    })
}

fn operation_name(route: &str, ordinal: usize) -> String {
    let name = route
        .trim_matches('/')
        .replace(['/', '.', '-'], "_")
        .trim_matches('_')
        .to_string();
    format!(
        "{}_{}",
        if name.is_empty() { "operation" } else { &name },
        ordinal
    )
}

fn is_write_route(route: &str) -> bool {
    WRITE_ROUTES.contains(&route)
}

fn requires_restoration(route: &str) -> bool {
    matches!(
        route,
        "/v1/group/set-member-mute"
            | "/v1/group/member-mute-cancel"
            | "/v1/group/set-member-nickname"
            | "nim.updateNickInTeam"
            | "/v1/group/set-group-mute"
            | "/v1/group/add-notice"
            | "/v1/group/notice-opt"
    )
}

fn state_key(route: &str, identity: &Value) -> String {
    format!("{}:{identity}", restoration_family(route))
}

fn restoration_family(route: &str) -> &str {
    match route {
        "/v1/group/set-member-mute" | "/v1/group/member-mute-cancel" => "member-mute",
        "/v1/group/set-member-nickname" | "nim.updateNickInTeam" => "member-rename",
        "/v1/group/add-notice" | "/v1/group/notice-opt" => "group-notice",
        "/v1/group/set-group-mute" => "group-mute",
        _ => route,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> CalibrationMetadata {
        CalibrationMetadata {
            app_file_version: "2.7.8".into(),
            main_script_sha256: "a".repeat(64),
            page_title: "旺商聊".into(),
            page_url: "file:///index.html".into(),
        }
    }

    #[test]
    fn only_selected_unverified_capabilities_are_allowed() {
        let mut recorder = DeveloperCalibrationRecorder::default();
        recorder.start(metadata(), vec!["mute".into()]).unwrap();
        assert!(recorder.allows("mute"));
        assert!(!recorder.allows("announcement"));
    }

    #[test]
    fn write_capture_requires_restoration_confirmation() {
        let mut recorder = DeveloperCalibrationRecorder::default();
        recorder.start(metadata(), vec!["mute".into()]).unwrap();
        recorder.require_baseline(
            "/v1/group/set-member-mute",
            json!({"groupId": 1, "userId": 2}),
        );
        recorder.record_ipc(
            "/v1/group/set-member-mute",
            "request",
            json!({"groupId": 1, "userId": 2, "min": 1}),
            &json!({
                "transportCode": 200,
                "errno": 0,
                "requestId": "request-1",
                "response": "{\"code\":0,\"data\":{},\"msg\":\"OK\"}"
            }),
        );
        recorder.record_callbacks(&[GatewayRecord {
            session: "session-1".into(),
            sequence: 1,
            kind: crate::gateway::GatewayRecordKind::TeamMemberUpdated,
            source: "onupdateteammember".into(),
            payload: json!({"teamId": "group-1"}),
        }]);
        assert_eq!(
            recorder.finish(false).unwrap_err().code,
            "calibration_not_finalizing"
        );
        assert!(recorder.status().active);
        recorder.record_restored_state(
            "/v1/group/member-mute-cancel",
            json!({"groupId": 1, "userId": 2}),
            json!({"muted": false}),
        );
        recorder.record_baseline(
            "/v1/group/set-member-mute",
            json!({"groupId": 1, "userId": 2}),
            json!({"muted": false}),
        );
        recorder.record_restored_state(
            "/v1/group/member-mute-cancel",
            json!({"groupId": 1, "userId": 2}),
            json!({"muted": false}),
        );
        recorder.begin_finalization().unwrap();
        assert!(recorder.status().finishing);
        assert!(!recorder.allows("mute"));
        assert_eq!(recorder.cancel().unwrap_err().code, "calibration_finishing");
        let targets = recorder.finalization_targets().unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(
            recorder.finish(true).unwrap_err().code,
            "calibration_not_restored"
        );
        recorder.record_restored_state(
            "/v1/group/member-mute-cancel",
            json!({"groupId": 1, "userId": 2}),
            json!({"muted": false}),
        );
        let exported = recorder.finish(true).unwrap();
        let baselines = exported.capture["expectedNormalizedState"]["baselines"]
            .as_array()
            .unwrap();
        assert_eq!(baselines.len(), 1);
        assert_eq!(baselines[0]["family"], "member-mute");
        assert_eq!(baselines[0]["identity"]["groupId"], 1);
        recorder.commit();
        assert!(!recorder.status().active);
    }
}

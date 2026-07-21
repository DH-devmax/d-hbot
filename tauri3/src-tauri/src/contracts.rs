#[cfg(test)]
mod tests {
    use serde_json::Value;

    fn fixture(raw: &str) -> Value {
        serde_json::from_str(raw).expect("contract fixture must be valid JSON")
    }

    #[test]
    fn frozen_contracts_have_explicit_versions() {
        let values = [
            fixture(include_str!("../../contracts/group_gateway_v1.json")),
            fixture(include_str!("../../contracts/ai_provider_v1.json")),
            fixture(include_str!("../../contracts/prediction_v1.json")),
            fixture(include_str!("../../contracts/schedule_v1.json")),
        ];
        assert!(values.iter().all(|value| value.get("version").is_some()));
    }
}

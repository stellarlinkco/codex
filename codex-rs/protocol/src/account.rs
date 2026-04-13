use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, JsonSchema, TS, Default)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum PlanType {
    #[default]
    Free,
    Go,
    Plus,
    Pro,
    ProLite,
    Team,
    Business,
    Enterprise,
    Edu,
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::PlanType;
    use pretty_assertions::assert_eq;

    #[test]
    fn prolite_uses_expected_wire_name() {
        assert_eq!(
            serde_json::to_string(&PlanType::ProLite).expect("prolite should serialize"),
            "\"prolite\""
        );
        assert_eq!(
            serde_json::from_str::<PlanType>("\"prolite\"").expect("prolite should deserialize"),
            PlanType::ProLite
        );
    }
}

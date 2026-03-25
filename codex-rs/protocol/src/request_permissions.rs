use crate::models::FileSystemPermissions;
use crate::models::MacOsSeatbeltProfileExtensions;
use crate::models::NetworkPermissions;
use crate::models::PermissionProfile;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum PermissionGrantScope {
    #[default]
    Turn,
    Session,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct RequestPermissionProfile {
    pub network: Option<NetworkPermissions>,
    pub file_system: Option<FileSystemPermissions>,
    pub macos: Option<MacOsSeatbeltProfileExtensions>,
}

impl RequestPermissionProfile {
    pub fn is_empty(&self) -> bool {
        self.network.is_none() && self.file_system.is_none() && self.macos.is_none()
    }
}

impl From<RequestPermissionProfile> for PermissionProfile {
    fn from(value: RequestPermissionProfile) -> Self {
        Self {
            network: value.network,
            file_system: value.file_system,
            macos: value.macos,
        }
    }
}

impl From<PermissionProfile> for RequestPermissionProfile {
    fn from(value: PermissionProfile) -> Self {
        Self {
            network: value.network,
            file_system: value.file_system,
            macos: value.macos,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::MacOsAutomationPermission;
    use crate::models::MacOsPreferencesPermission;
    use pretty_assertions::assert_eq;

    #[test]
    fn request_permission_profile_round_trips_macos_permissions() {
        let profile = RequestPermissionProfile {
            macos: Some(MacOsSeatbeltProfileExtensions {
                macos_preferences: MacOsPreferencesPermission::ReadWrite,
                macos_automation: MacOsAutomationPermission::BundleIds(vec![
                    "com.apple.Notes".to_string(),
                ]),
                macos_launch_services: true,
                macos_accessibility: true,
                macos_calendar: false,
                macos_reminders: false,
                macos_contacts: Default::default(),
            }),
            ..Default::default()
        };

        assert_eq!(
            RequestPermissionProfile::from(PermissionProfile::from(profile.clone())),
            profile
        );
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestPermissionsArgs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub permissions: RequestPermissionProfile,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestPermissionsResponse {
    pub permissions: RequestPermissionProfile,
    #[serde(default)]
    pub scope: PermissionGrantScope,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct RequestPermissionsEvent {
    /// Responses API call id for the associated tool call, if available.
    pub call_id: String,
    /// Turn ID that this request belongs to.
    /// Uses `#[serde(default)]` for backwards compatibility.
    #[serde(default)]
    pub turn_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub permissions: RequestPermissionProfile,
}

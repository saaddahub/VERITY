//! Session profiles (docs/06-session-mode.md, "Profiles"): one default and an
//! optional per-game override, stored as JSON.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Session actions a profile may mention (docs/05-actions.md catalog).
/// Ids outside this list are dropped with a warning, never an error.
pub const SESSION_ACTIONS: [&str; 7] = [
    "power.session_overlay",
    "sched.background_ecoqos",
    "sched.background_cpusets",
    "sched.game_priority",
    "power.session_scheme",
    "services.session_pause",
    "memory.standby_purge",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionSetting {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
    /// Experiment id when a Lab result enabled it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proven_by: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub version: u32,
    pub capture_mode: String,
    pub actions: BTreeMap<String, ActionSetting>,
    #[serde(default)]
    pub extra_exclusions: Vec<String>,
    #[serde(default)]
    pub notes: String,
}

impl Default for Profile {
    /// The spec's default profile.
    fn default() -> Self {
        let on = |enabled| ActionSetting {
            enabled,
            params: Value::Null,
            proven_by: None,
        };
        let actions = SESSION_ACTIONS
            .iter()
            .map(|id| {
                let enabled = matches!(
                    *id,
                    "power.session_overlay"
                        | "sched.background_ecoqos"
                        | "sched.background_cpusets"
                );
                (id.to_string(), on(enabled))
            })
            .collect();
        Self {
            version: 1,
            capture_mode: "standard".into(),
            actions,
            extra_exclusions: Vec::new(),
            notes: String::new(),
        }
    }
}

/// Longest notes field and exclusion list accepted from a client.
const MAX_NOTES: usize = 4096;
const MAX_EXCLUSIONS: usize = 256;

/// Parses and validates a profile from a client. Returns the profile and the
/// warnings for anything dropped.
pub fn parse(v: Value) -> Result<(Profile, Vec<String>), String> {
    let mut p: Profile = serde_json::from_value(v).map_err(|e| format!("invalid profile: {e}"))?;
    if p.version != 1 {
        return Err(format!("unsupported profile version {}", p.version));
    }
    if !matches!(p.capture_mode.as_str(), "light" | "standard" | "deep") {
        return Err(format!(
            "capture_mode must be light, standard or deep, not {}",
            p.capture_mode
        ));
    }
    if p.notes.len() > MAX_NOTES {
        return Err(format!("notes longer than {MAX_NOTES} bytes"));
    }
    if p.extra_exclusions.len() > MAX_EXCLUSIONS {
        return Err(format!("more than {MAX_EXCLUSIONS} exclusions"));
    }
    for name in &p.extra_exclusions {
        let ok = name.to_ascii_lowercase().ends_with(".exe")
            && !name.contains(['\\', '/', ':'])
            && name.len() <= 260;
        if !ok {
            return Err(format!(
                "exclusion {name:?} must be an executable file name like obs64.exe"
            ));
        }
    }
    let mut warnings = Vec::new();
    p.actions.retain(|id, _| {
        let known = SESSION_ACTIONS.contains(&id.as_str());
        if !known {
            warnings.push(format!("unknown action {id} ignored"));
        }
        known
    });
    Ok((p, warnings))
}

/// A stored profile; unreadable or missing falls back to the default.
pub fn load(json: Option<&str>) -> Profile {
    json.and_then(|j| serde_json::from_str::<Value>(j).ok())
        .and_then(|v| parse(v).ok())
        .map(|(p, _)| p)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spec_example_parses_and_unknown_ids_warn() {
        let v = json!({
            "version": 1,
            "capture_mode": "standard",
            "actions": {
                "power.session_overlay": { "enabled": true },
                "services.session_pause": { "enabled": false, "params": { "services": [] } },
                "made.up": { "enabled": true }
            },
            "extra_exclusions": ["obs64.exe"],
            "notes": ""
        });
        let (p, warnings) = parse(v).unwrap();
        assert_eq!(warnings, ["unknown action made.up ignored"]);
        assert_eq!(p.actions.len(), 2);
        assert!(p.actions["power.session_overlay"].enabled);
    }

    #[test]
    fn invalid_profiles_are_rejected() {
        let base = || serde_json::to_value(Profile::default()).unwrap();
        let mut v = base();
        v["version"] = json!(2);
        assert!(parse(v).is_err());
        let mut v = base();
        v["capture_mode"] = json!("turbo");
        assert!(parse(v).is_err());
        let mut v = base();
        v["extra_exclusions"] = json!([r"C:\evil\x.exe"]);
        assert!(parse(v).is_err());
        let mut v = base();
        v["notes"] = json!("x".repeat(5000));
        assert!(parse(v).is_err());
        assert!(parse(json!({"version": 1})).is_err());
    }

    #[test]
    fn default_round_trips_and_load_falls_back() {
        let d = Profile::default();
        let (back, w) = parse(serde_json::to_value(&d).unwrap()).unwrap();
        assert_eq!(back, d);
        assert!(w.is_empty());
        assert_eq!(load(None), d);
        assert_eq!(load(Some("not json")), d);
        assert!(d.actions["sched.background_ecoqos"].enabled);
        assert!(!d.actions["memory.standby_purge"].enabled);
    }
}

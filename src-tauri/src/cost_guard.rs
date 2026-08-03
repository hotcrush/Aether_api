use crate::db::Db;
use serde::{Deserialize, Serialize};

pub const COST_GUARD_SETTING_KEY: &str = "aether:cost_guard";
const MAX_COST_MULTIPLIER: f64 = 100.0;
const MAX_SAFETY_BUFFER: f64 = 0.95;

/// Local equivalent of Sub2API's group profit gate. Aether has no resale
/// groups, so users set the acceptable upstream cost ceiling directly.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostGuardSettings {
    pub enabled: bool,
    pub max_cost_multiplier: f64,
    pub safety_buffer: f64,
}

impl Default for CostGuardSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            max_cost_multiplier: 1.0,
            safety_buffer: 0.0,
        }
    }
}

impl CostGuardSettings {
    pub fn validate(self) -> Result<Self, String> {
        if !self.max_cost_multiplier.is_finite()
            || !(0.0..=MAX_COST_MULTIPLIER).contains(&self.max_cost_multiplier)
        {
            return Err(format!(
                "最大成本倍率必须在 0 到 {MAX_COST_MULTIPLIER} 之间"
            ));
        }
        if !self.safety_buffer.is_finite()
            || !(0.0..=MAX_SAFETY_BUFFER).contains(&self.safety_buffer)
        {
            return Err("安全缓冲必须在 0% 到 95% 之间".to_string());
        }
        Ok(self)
    }

    pub fn allows(&self, rate_multiplier: f64) -> bool {
        !self.enabled
            || (rate_multiplier.is_finite()
                && rate_multiplier >= 0.0
                && rate_multiplier
                    <= self.max_cost_multiplier * (1.0 - self.safety_buffer) + f64::EPSILON)
    }
}

pub fn load(db: &Db) -> CostGuardSettings {
    db.get_setting(COST_GUARD_SETTING_KEY)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str::<CostGuardSettings>(&value).ok())
        .and_then(|settings| settings.validate().ok())
        .unwrap_or_default()
}

pub fn save(db: &Db, settings: CostGuardSettings) -> Result<CostGuardSettings, String> {
    let settings = settings.validate()?;
    let encoded = serde_json::to_string(&settings).map_err(|error| error.to_string())?;
    db.set_setting(COST_GUARD_SETTING_KEY, &encoded)
        .map_err(|error| error.to_string())?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::CostGuardSettings;

    #[test]
    fn applies_cost_ceiling_and_safety_buffer() {
        let settings = CostGuardSettings {
            enabled: true,
            max_cost_multiplier: 1.5,
            safety_buffer: 0.1,
        };
        assert!(settings.allows(1.35));
        assert!(!settings.allows(1.36));
    }
}

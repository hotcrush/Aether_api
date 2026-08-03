use crate::db::Db;
use serde::{Deserialize, Serialize};

const OUTBOUND_PROXY_SETTING_KEY: &str = "aether:outbound_proxy";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "snake_case")]
pub struct OutboundProxySettings {
    pub enabled: bool,
    pub url: String,
}

impl Default for OutboundProxySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "http://127.0.0.1:7890".to_string(),
        }
    }
}

impl OutboundProxySettings {
    pub fn validate(mut self) -> Result<Self, String> {
        self.url = self.url.trim().to_string();
        if !self.enabled {
            return Ok(self);
        }
        if self.url.is_empty() {
            return Err("请填写出站代理地址".to_string());
        }
        let parsed =
            reqwest::Url::parse(&self.url).map_err(|_| "出站代理地址格式无效".to_string())?;
        if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h") {
            return Err("出站代理仅支持 HTTP、HTTPS、SOCKS5 或 SOCKS5H 地址".to_string());
        }
        if parsed.host_str().is_none() {
            return Err("出站代理地址缺少主机名或 IP".to_string());
        }
        Ok(self)
    }
}

pub fn load(db: &Db) -> OutboundProxySettings {
    db.get_setting(OUTBOUND_PROXY_SETTING_KEY)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str::<OutboundProxySettings>(&value).ok())
        .and_then(|settings| settings.validate().ok())
        .unwrap_or_default()
}

pub fn save(db: &Db, settings: OutboundProxySettings) -> Result<OutboundProxySettings, String> {
    let settings = settings.validate()?;
    let encoded = serde_json::to_string(&settings).map_err(|error| error.to_string())?;
    db.set_setting(OUTBOUND_PROXY_SETTING_KEY, &encoded)
        .map_err(|error| error.to_string())?;
    Ok(settings)
}

pub fn build_client(
    timeout_secs: u64,
    connect_timeout_secs: u64,
    settings: &OutboundProxySettings,
) -> Result<reqwest::Client, String> {
    crate::dns::build_client(
        timeout_secs,
        connect_timeout_secs,
        settings.enabled.then_some(settings.url.as_str()),
    )
}

#[cfg(test)]
mod tests {
    use super::OutboundProxySettings;

    #[test]
    fn validates_supported_proxy_urls() {
        assert!(OutboundProxySettings {
            enabled: true,
            url: "http://127.0.0.1:7890".to_string(),
        }
        .validate()
        .is_ok());
        assert!(OutboundProxySettings {
            enabled: true,
            url: "socks5h://127.0.0.1:7891".to_string(),
        }
        .validate()
        .is_ok());
        assert!(OutboundProxySettings {
            enabled: true,
            url: "127.0.0.1:7890".to_string(),
        }
        .validate()
        .is_err());
    }
}

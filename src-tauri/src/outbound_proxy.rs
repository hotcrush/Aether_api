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

/// Convert the configured outbound proxy into the schemes supported by Wry/Tauri WebViews.
/// WebView2 accepts HTTP and SOCKS5 proxy endpoints; SOCKS5H is normalized because Chromium
/// resolves hostnames through its SOCKS proxy. HTTPS proxy transport is not supported by Wry.
pub fn webview_proxy_url(settings: &OutboundProxySettings) -> Result<Option<tauri::Url>, String> {
    if !settings.enabled {
        return Ok(None);
    }
    let settings = settings.clone().validate()?;
    let mut url = settings
        .url
        .parse::<tauri::Url>()
        .map_err(|_| "出站代理地址格式无效".to_string())?;
    match url.scheme() {
        "http" | "socks5" => {}
        "socks5h" => {
            url.set_scheme("socks5")
                .map_err(|_| "无法为内置授权页启用 SOCKS5 代理".to_string())?;
        }
        "https" => {
            return Err(
                "内置授权页暂不支持 HTTPS 代理，请改用 HTTP、SOCKS5 或 SOCKS5H 代理".to_string(),
            )
        }
        _ => return Err("内置授权页的代理协议不受支持".to_string()),
    }
    Ok(Some(url))
}

#[cfg(test)]
mod tests {
    use super::{webview_proxy_url, OutboundProxySettings};

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

    #[test]
    fn builds_webview_proxy_urls_without_silent_direct_fallback() {
        let disabled = OutboundProxySettings::default();
        assert!(webview_proxy_url(&disabled).unwrap().is_none());

        let http = OutboundProxySettings {
            enabled: true,
            url: "http://127.0.0.1:7890".to_string(),
        };
        assert_eq!(
            webview_proxy_url(&http).unwrap().unwrap().as_str(),
            "http://127.0.0.1:7890/"
        );

        let socks5h = OutboundProxySettings {
            enabled: true,
            url: "socks5h://127.0.0.1:7891".to_string(),
        };
        assert_eq!(
            webview_proxy_url(&socks5h).unwrap().unwrap().scheme(),
            "socks5"
        );

        let https = OutboundProxySettings {
            enabled: true,
            url: "https://proxy.example.com:443".to_string(),
        };
        assert!(webview_proxy_url(&https).is_err());
    }
}

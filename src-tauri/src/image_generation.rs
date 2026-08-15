use crate::db::{Account, Db};
use serde::{Deserialize, Serialize};

const IMAGE_GENERATION_SETTING_KEY: &str = "aether:image_generation";
pub(crate) const DEDICATED_ACCOUNT_ID: &str = "__aether_image_generation__";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "snake_case")]
pub struct ImageGenerationSettings {
    pub enabled: bool,
    pub base_url: String,
    pub api_key: String,
}

impl Default for ImageGenerationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
        }
    }
}

impl ImageGenerationSettings {
    pub fn validate(mut self) -> Result<Self, String> {
        self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
        self.api_key = self.api_key.trim().to_string();
        if !self.enabled {
            return Ok(self);
        }
        if self.base_url.is_empty() {
            return Err("请填写图片生成上游地址".to_string());
        }
        let parsed = reqwest::Url::parse(&self.base_url)
            .map_err(|_| "图片生成上游地址格式无效".to_string())?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err("图片生成上游仅支持 HTTP 或 HTTPS 地址".to_string());
        }
        if parsed.host_str().is_none() {
            return Err("图片生成上游地址缺少主机名或 IP".to_string());
        }
        if self.api_key.is_empty() {
            return Err("请填写图片生成上游 API Key".to_string());
        }
        Ok(self)
    }
}

pub fn load(db: &Db) -> ImageGenerationSettings {
    db.get_setting(IMAGE_GENERATION_SETTING_KEY)
        .ok()
        .flatten()
        .and_then(|value| serde_json::from_str::<ImageGenerationSettings>(&value).ok())
        .and_then(|settings| settings.validate().ok())
        .unwrap_or_default()
}

pub fn save(db: &Db, settings: ImageGenerationSettings) -> Result<ImageGenerationSettings, String> {
    let settings = settings.validate()?;
    let encoded = serde_json::to_string(&settings).map_err(|error| error.to_string())?;
    db.set_setting(IMAGE_GENERATION_SETTING_KEY, &encoded)
        .map_err(|error| error.to_string())?;
    Ok(settings)
}

pub fn dedicated_account(settings: &ImageGenerationSettings) -> Account {
    Account {
        id: DEDICATED_ACCOUNT_ID.to_string(),
        name: "图片生成上游".to_string(),
        account_type: "api_key".to_string(),
        api_key: settings.api_key.clone(),
        access_token: String::new(),
        refresh_token: String::new(),
        refreshable: false,
        id_token: String::new(),
        client_id: String::new(),
        credential_masked: "专用图片生成 Key".to_string(),
        base_url: settings.base_url.clone(),
        chatgpt_account_id: String::new(),
        chatgpt_user_id: String::new(),
        email: String::new(),
        plan_type: String::new(),
        expires_at: None,
        priority: 0,
        models: Vec::new(),
        weight: 1,
        concurrency: 100,
        rate_multiplier: 1.0,
        auto_sync_rate_multiplier: false,
        locked: false,
        status: "active".to_string(),
        last_error: String::new(),
        last_used_at: None,
        request_count: 0,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

pub fn is_dedicated_account(account: &Account) -> bool {
    account.id == DEDICATED_ACCOUNT_ID
}

pub fn is_dedicated_account_id(account_id: &str) -> bool {
    account_id == DEDICATED_ACCOUNT_ID
}

#[cfg(test)]
mod tests {
    use super::ImageGenerationSettings;

    #[test]
    fn validates_direct_and_relay_urls() {
        let direct = ImageGenerationSettings {
            enabled: true,
            base_url: "https://api.openai.com/v1/".to_string(),
            api_key: " sk-test ".to_string(),
        }
        .validate()
        .unwrap();
        assert_eq!(direct.base_url, "https://api.openai.com/v1");
        assert_eq!(direct.api_key, "sk-test");

        assert!(ImageGenerationSettings {
            enabled: true,
            base_url: "socks5://127.0.0.1:1080".to_string(),
            api_key: "sk-test".to_string(),
        }
        .validate()
        .is_err());
    }
}

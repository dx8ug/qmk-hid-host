use std::{path::PathBuf, sync::OnceLock};

#[derive(serde::Deserialize, serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WeatherConfig {
    pub url: String,
}

#[derive(Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Providers {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<ProviderEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<ProviderEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<ProviderEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<ProviderEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay: Option<ProviderEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ProviderEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weather: Option<WeatherEntry>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderEntry {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WeatherEntry {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub url: String,
}

fn default_true() -> bool {
    true
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub devices: Vec<Device>,
    pub layouts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconnect_delay: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weather: Option<WeatherConfig>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(serialize_with = "hex_to_string", deserialize_with = "string_to_hex")]
    pub vendor_id: u16,
    #[serde(serialize_with = "hex_to_string", deserialize_with = "string_to_hex")]
    pub product_id: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_page: Option<u16>,
}

static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn get_config() -> &'static Config {
    CONFIG.get().unwrap()
}

pub fn load_config(path: PathBuf) -> &'static Config {
    if let Some(config) = CONFIG.get() {
        return config;
    }

    let default_config = Config {
        devices: vec![Device {
            name: None,
            vendor_id: 0x0,
            product_id: 0x0844,
            usage: None,
            usage_page: None,
        }],
        layouts: vec!["en".to_string()],
        reconnect_delay: None,
        weather: Some(WeatherConfig {
            url: "wttr.in/Hamburg?format=%t".to_string(),
        }),
    };

    if let Ok(file) = std::fs::read_to_string(&path) {
        let config = serde_json::from_str::<Config>(&file)
            .map_err(|e| tracing::error!("Incorrect config file: {}", e))
            .unwrap();
        return CONFIG.get_or_init(|| config);
    }

    let file_content = serde_json::to_string_pretty(&default_config).unwrap();
    std::fs::write(&path, &file_content)
        .map_err(|e| tracing::error!("Error while saving config file to {:?}: {}", path, e))
        .unwrap();
    tracing::info!("New config file created at {:?}", path);

    CONFIG.get_or_init(|| default_config)
}

fn string_to_hex<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: &str = serde::Deserialize::deserialize(deserializer)?;
    let hex = value.trim_start_matches("0x");
    return u16::from_str_radix(hex, 16).map_err(serde::de::Error::custom);
}

fn hex_to_string<S>(value: &u16, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&format!("0x{:04x}", value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_providers_defaults_to_all_none() {
        let p: Providers = serde_json::from_str("{}").unwrap();
        assert!(p.time.is_none());
        assert!(p.weather.is_none());
    }

    #[test]
    fn provider_entry_enabled_defaults_to_true() {
        let p: Providers = serde_json::from_str(r#"{"media": {}}"#).unwrap();
        assert!(p.media.unwrap().enabled);
    }

    #[test]
    fn provider_entry_explicit_false_is_disabled() {
        let p: Providers = serde_json::from_str(r#"{"media": {"enabled": false}}"#).unwrap();
        assert!(!p.media.unwrap().enabled);
    }

    #[test]
    fn unknown_provider_name_rejected() {
        let err = serde_json::from_str::<Providers>(r#"{"tiem": {}}"#);
        assert!(err.is_err(), "expected serde error on unknown provider 'tiem'");
    }

    #[test]
    fn unknown_field_inside_provider_entry_rejected() {
        let err = serde_json::from_str::<Providers>(r#"{"media": {"enable": false}}"#);
        assert!(err.is_err(), "expected serde error on misspelled 'enable'");
    }

    #[test]
    fn weather_requires_url() {
        let err = serde_json::from_str::<Providers>(r#"{"weather": {"enabled": true}}"#);
        assert!(err.is_err(), "weather without url must fail");
    }

    #[test]
    fn weather_with_url_parses() {
        let p: Providers = serde_json::from_str(r#"{"weather": {"url": "wttr.in/X?format=%t"}}"#).unwrap();
        let w = p.weather.unwrap();
        assert!(w.enabled);
        assert_eq!(w.url, "wttr.in/X?format=%t");
    }
}

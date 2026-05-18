use std::{path::PathBuf, sync::OnceLock};

#[derive(Default, PartialEq, serde::Deserialize, serde::Serialize)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streamdeck: Option<StreamDeckEntry>,
}

#[derive(PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderEntry {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WeatherEntry {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub url: String,
}

fn default_true() -> bool {
    true
}

#[derive(PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamDeckEntry {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_streamdeck_port")]
    pub port: u16,
}

fn default_streamdeck_port() -> u16 {
    6543
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub devices: Vec<Device>,
    pub layouts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconnect_delay: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_level: Option<String>,
    #[serde(default, skip_serializing_if = "providers_is_empty")]
    pub providers: Providers,
}

fn providers_is_empty(p: &Providers) -> bool {
    p == &Providers::default()
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
        log_level: None,
        providers: Providers::default(),
    };

    // load_config runs before tracing is initialised (level comes from this very config),
    // so failures here panic with the full error in the panic message rather than going through tracing.
    if let Ok(file) = std::fs::read_to_string(&path) {
        let config = serde_json::from_str::<Config>(&file).unwrap_or_else(|e| panic!("Incorrect config file {:?}: {}", path, e));
        return CONFIG.get_or_init(|| config);
    }

    let file_content = serde_json::to_string_pretty(&default_config).expect("Failed to serialize default config");
    std::fs::write(&path, &file_content).unwrap_or_else(|e| panic!("Error while saving config file to {:?}: {}", path, e));

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

    #[test]
    fn old_top_level_weather_field_rejected() {
        let json = r#"{
            "devices": [{"vendorId": "0x0001", "productId": "0x0002"}],
            "layouts": [],
            "weather": {"url": "wttr.in/X?format=%t"}
        }"#;
        let err = serde_json::from_str::<Config>(json);
        assert!(err.is_err(), "old top-level 'weather' must be rejected by deny_unknown_fields");
    }

    #[test]
    fn streamdeck_enabled_defaults_to_false() {
        let p: Providers = serde_json::from_str(r#"{"streamdeck": {}}"#).unwrap();
        let s = p.streamdeck.unwrap();
        assert!(!s.enabled);
        assert_eq!(s.port, 6543);
    }

    #[test]
    fn streamdeck_custom_port() {
        let p: Providers = serde_json::from_str(r#"{"streamdeck": {"enabled": true, "port": 12000}}"#).unwrap();
        let s = p.streamdeck.unwrap();
        assert!(s.enabled);
        assert_eq!(s.port, 12000);
    }

    #[test]
    fn streamdeck_unknown_field_rejected() {
        let err = serde_json::from_str::<Providers>(r#"{"streamdeck": {"prt": 1234}}"#);
        assert!(err.is_err(), "misspelled 'prt' must be rejected");
    }

    #[test]
    fn streamdeck_bind_field_rejected() {
        let err = serde_json::from_str::<Providers>(r#"{"streamdeck": {"bind": "0.0.0.0"}}"#);
        assert!(err.is_err(), "legacy 'bind' field must be rejected — bridge is loopback-only");
    }

    #[test]
    fn log_level_field_parses() {
        let json = r#"{
            "devices": [{"vendorId": "0x0001", "productId": "0x0002"}],
            "layouts": [],
            "logLevel": "warn"
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.log_level.as_deref(), Some("warn"));
    }

    #[test]
    fn log_level_absent_is_none() {
        let json = r#"{
            "devices": [{"vendorId": "0x0001", "productId": "0x0002"}],
            "layouts": []
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.log_level.is_none());
    }

    #[test]
    fn config_with_providers_section_parses() {
        let json = r#"{
            "devices": [{"vendorId": "0x0001", "productId": "0x0002"}],
            "layouts": [],
            "providers": {
                "media": {"enabled": false},
                "weather": {"url": "wttr.in/X?format=%t"}
            }
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(!cfg.providers.media.unwrap().enabled);
        assert_eq!(cfg.providers.weather.unwrap().url, "wttr.in/X?format=%t");
    }
}

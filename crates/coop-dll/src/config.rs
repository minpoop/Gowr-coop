//! `gowr-coop.toml`, next to the DLL in the game folder.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Mod loaded but not networking (for checking offsets with the overlay/log).
    Off,
    Host,
    Join,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoopConfig {
    pub role: Role,
    #[serde(default = "default_name")]
    pub name: String,
    /// Host: UDP port to listen on (forward it on your router).
    #[serde(default = "default_port")]
    pub port: u16,
    /// Join: the host's address, e.g. "203.0.113.5:27015".
    #[serde(default)]
    pub host_address: Option<String>,
    #[serde(default = "default_max_players")]
    pub max_players: usize,
}

fn default_name() -> String {
    "Kratos".into()
}
fn default_port() -> u16 {
    27015
}
fn default_max_players() -> usize {
    2
}

impl CoopConfig {
    pub fn parse(text: &str) -> Result<Self, String> {
        let cfg: CoopConfig = toml::from_str(text).map_err(|e| e.to_string())?;
        if cfg.role == Role::Join && cfg.host_address.is_none() {
            return Err("role = \"join\" needs host_address = \"ip:port\"".into());
        }
        if !(2..=4).contains(&cfg.max_players) {
            return Err("max_players must be 2 to 4".into());
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses() {
        let cfg = CoopConfig::parse(include_str!("../../../config/gowr-coop.example.toml")).unwrap();
        assert_eq!(cfg.role, Role::Host);
        assert_eq!(cfg.port, 27015);
    }

    #[test]
    fn join_needs_an_address() {
        assert!(CoopConfig::parse("role = \"join\"").is_err());
        let cfg = CoopConfig::parse("role = \"join\"\nhost_address = \"1.2.3.4:27015\"\nname = \"Atreus\"").unwrap();
        assert_eq!(cfg.host_address.as_deref(), Some("1.2.3.4:27015"));
    }

    #[test]
    fn typos_are_errors_not_silently_ignored() {
        assert!(CoopConfig::parse("role = \"host\"\nprot = 1").is_err());
    }
}

//! Integration tests for core types and config parsing.

use orchestrator::config::Config;

#[test]
fn example_config_parses() {
    let yaml = include_str!("../config.example.yaml");
    let expanded = shellexpand::env_with_context(
        yaml,
        |k| -> Result<Option<std::borrow::Cow<'_, str>>, std::env::VarError> {
            Ok(Some(
                std::env::var(k)
                    .ok()
                    .map(std::convert::Into::into)
                    .unwrap_or_else(|| std::borrow::Cow::Borrowed("")),
            ))
        },
    )
    .expect("expand env vars")
    .into_owned();
    let cfg: Config = serde_yaml::from_str(&expanded).expect("parse config");
    assert!(!cfg.providers.is_empty());
    assert!(!cfg.runtimes.is_empty());
    assert!(!cfg.models.is_empty());
    assert!(!cfg.roles.is_empty());
}

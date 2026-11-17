use figment::{
    Figment,
    providers::{Format, Toml},
};
use stories_config::Settings;

fn settings() -> Result<Settings, Box<dyn std::error::Error>> {
    Ok(Settings::from_figment(
        Figment::new()
            .merge(Toml::string(include_str!("../../../config/exemplo.toml")))
            .merge(Toml::string(include_str!("../../../config/sources.toml"))),
    )?)
}

#[test]
fn dry_run_needs_no_external_credentials_and_redacts_secrets()
-> Result<(), Box<dyn std::error::Error>> {
    let config = settings()?;
    let secrets = config.resolve_with(|name| match name {
        "DATABASE_URL" => Some("postgres://user:confidencial@localhost/stories".into()),
        "ADMIN_TOKEN" => Some("token-secreto-com-mais-de-trinta-e-dois-bytes".into()),
        _ => None,
    })?;
    assert!(secrets.instagram_token.is_none());
    let debug = format!("{secrets:?}");
    assert!(!debug.contains("confidencial"));
    assert!(!debug.contains("token-secreto"));
    Ok(())
}

#[test]
fn bounds_and_plaintext_credentials_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = settings()?;
    config.pipeline.daily_cap = 81;
    assert!(config.validate().is_err());
    config.pipeline.daily_cap = 40;
    config.instagram.access_token = "credencial-em-texto".into();
    let error = config.validate().err().ok_or("validação deveria falhar")?;
    assert!(!error.to_string().contains("credencial-em-texto"));
    Ok(())
}

#[test]
fn live_mode_requires_external_credentials() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = settings()?;
    config.instagram.dry_run = false;
    assert!(
        config
            .resolve_with(|name| match name {
                "DATABASE_URL" => Some("postgres://localhost/stories".into()),
                "ADMIN_TOKEN" => Some("a".repeat(32)),
                _ => None,
            })
            .is_err()
    );
    Ok(())
}

#[test]
fn unknown_fields_and_short_admin_tokens_fail() -> Result<(), Box<dyn std::error::Error>> {
    let text = include_str!("../../../config/exemplo.toml")
        .replace("dry_run = true", "dry_run = true\nunknown = 1");
    assert!(Settings::from_figment(Figment::new().merge(Toml::string(&text))).is_err());
    assert!(settings()?.resolve_with(|_| Some("curto".into())).is_err());
    Ok(())
}

#[test]
fn editorial_requires_key_even_in_dry_run_and_valid_origin_groups()
-> Result<(), Box<dyn std::error::Error>> {
    let mut config = settings()?;
    config.llm.enabled = true;
    let base = |name: &str| match name {
        "DATABASE_URL" => Some("postgres://localhost/stories".into()),
        "ADMIN_TOKEN" => Some("a".repeat(32)),
        _ => None,
    };
    assert!(config.resolve_with(base).is_err());
    let secrets = config.resolve_with(|name| {
        if name == "LLM_API_KEY" {
            Some("fixture-only-key".into())
        } else {
            base(name)
        }
    })?;
    assert!(!format!("{secrets:?}").contains("fixture-only-key"));
    config.sources[0].origin_group = Some(" ".into());
    assert!(config.validate().is_err());
    config.sources[0].origin_group = Some("Rust Foundation".into());
    config.validate()?;
    Ok(())
}

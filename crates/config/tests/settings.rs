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

#[test]
fn imaging_limits_paths_and_manifest_are_validated() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config = Settings::load(
        &root.join("config/exemplo.toml"),
        &root.join("config/sources.toml"),
    )?;
    assert!(std::path::Path::new(&config.imaging.manifest).is_file());
    for change in 0..4 {
        let mut config = settings()?;
        match change {
            0 => config.imaging.max_pixels = 16_000_001,
            1 => config.imaging.max_bytes = 8 * 1024 * 1024 + 1,
            2 => config.imaging.allowed_domains = vec!["*.example.com".into()],
            _ => config.imaging.artifacts_dir = "../outside".into(),
        }
        assert!(config.validate().is_err());
    }
    Ok(())
}

#[test]
fn live_publisher_requires_r2_but_disabled_llm_needs_no_key()
-> Result<(), Box<dyn std::error::Error>> {
    let mut config = settings()?;
    config.instagram.dry_run = false;
    let secrets = config.resolve_with(|name| match name {
        "DATABASE_URL" => Some("postgres://localhost/stories".into()),
        "ADMIN_TOKEN" => Some("a".repeat(32)),
        "IG_USER_ID" | "META_APP_ID" => Some("123".into()),
        "IG_ACCESS_TOKEN"
        | "META_INSPECTION_TOKEN"
        | "R2_ACCESS_KEY_ID"
        | "R2_SECRET_ACCESS_KEY" => Some("fixture-only".into()),
        "R2_ACCOUNT_ID" => Some("a".repeat(32)),
        _ => None,
    })?;
    assert!(secrets.llm_key.is_none());
    assert!(secrets.r2_secret.is_some());
    assert!(!format!("{secrets:?}").contains("fixture-only"));
    Ok(())
}

#[test]
fn token_credentials_follow_login_and_timing_bounds() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = settings()?;
    config.instagram.dry_run = false;
    config.instagram.login = stories_config::Login::Instagram;
    let lookup = |name: &str| match name {
        "DATABASE_URL" => Some("postgres://localhost/stories".into()),
        "ADMIN_TOKEN" => Some("a".repeat(32)),
        "IG_USER_ID" => Some("123".into()),
        "IG_ACCESS_TOKEN" | "R2_ACCESS_KEY_ID" | "R2_SECRET_ACCESS_KEY" => {
            Some("fixture-only".into())
        }
        "R2_ACCOUNT_ID" => Some("a".repeat(32)),
        "IG_TOKEN_RECEIPT" => Some("fixture-receipt".into()),
        _ => None,
    };
    let secrets = config.resolve_with(lookup)?;
    assert!(secrets.meta_inspection_token.is_none());
    assert!(secrets.instagram_token_receipt.is_some());
    assert!(
        config
            .resolve_with(|name| if name == "IG_TOKEN_RECEIPT" {
                None
            } else {
                lookup(name)
            })
            .is_err()
    );
    config.instagram.login = stories_config::Login::Facebook;
    assert!(config.resolve_with(lookup).is_err());
    config.instagram.dry_run = true;
    config.resolve_with(|name| match name {
        "DATABASE_URL" => Some("postgres://localhost/stories".into()),
        "ADMIN_TOKEN" => Some("a".repeat(32)),
        _ => None,
    })?;
    for invalid in [0, 59, 301] {
        config.instagram.token.check_interval_s = invalid;
        assert!(config.validate().is_err());
    }
    config.instagram.token.check_interval_s = 300;
    config.instagram.token.refresh_days = 31;
    assert!(config.validate().is_err());
    config.instagram.token.refresh_days = 30;
    config.instagram.token.inspection_token = "plaintext-secret".into();
    assert!(config.validate().is_err());
    Ok(())
}

#[test]
fn correction_windows_and_bounds_are_validated() -> Result<(), Box<dyn std::error::Error>> {
    let config = settings()?;
    assert!(!config.corrections.enabled);
    assert_eq!(config.corrections.window_hours, 24);
    assert_eq!(config.corrections.high_impact_hours, 72);
    for mutation in 0..8 {
        let mut bad = config.clone();
        match mutation {
            0 => bad.corrections.interval_s = 59,
            1 => bad.corrections.interval_s = 3601,
            2 => bad.corrections.window_hours = 0,
            3 => bad.corrections.high_impact_hours = 23,
            4 => bad.corrections.high_impact_hours = 169,
            5 => bad.corrections.high_impact_score = f64::NAN,
            6 => bad.corrections.max_sources = 0,
            _ => bad.corrections.max_sources = 5,
        };
        assert!(bad.validate().is_err());
    }
    Ok(())
}

#[test]
fn priority_weights_must_form_a_finite_distribution() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = settings()?;
    for weights in [
        [0.0; 5],
        [1.0; 5],
        [f64::NAN, 0.2, 0.2, 0.2, 0.2],
        [-0.1, 0.3, 0.3, 0.3, 0.2],
    ] {
        config.publishing.priority_weights = weights;
        assert!(config.validate().is_err());
    }
    config.publishing.priority_weights = [1.0, 0.0, 0.0, 0.0, 0.0];
    config.validate()?;
    Ok(())
}

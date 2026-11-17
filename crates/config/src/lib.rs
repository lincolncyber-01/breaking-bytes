//! Configuração sem valores secretos em TOML, Debug ou mensagens de erro.

use figment::{
    Figment,
    providers::{Env, Format, Toml},
};
use serde::Deserialize;
use std::{collections::HashSet, fmt, net::SocketAddr, path::Path};
use thiserror::Error;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    #[serde(default)]
    pub collection: Collection,
    #[serde(default)]
    pub imaging: Imaging,
    #[serde(default)]
    pub publishing: Publishing,
    pub pipeline: Pipeline,
    pub database: Database,
    pub admin: Admin,
    pub instagram: Instagram,
    pub storage: Storage,
    pub llm: Llm,
    #[serde(default)]
    pub sources: Vec<Source>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Publishing {
    pub enabled: bool,
    pub reserved_slots: u32,
}
impl Default for Publishing {
    fn default() -> Self {
        Self {
            enabled: true,
            reserved_slots: 5,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Imaging {
    pub enabled: bool,
    pub manifest: String,
    pub allowed_domains: Vec<String>,
    pub artifacts_dir: String,
    pub max_bytes: usize,
    pub max_pixels: u64,
}
impl Default for Imaging {
    fn default() -> Self {
        Self {
            enabled: true,
            manifest: "images.json".into(),
            allowed_domains: Vec::new(),
            artifacts_dir: "artifacts".into(),
            max_bytes: 8 * 1024 * 1024,
            max_pixels: 16_000_000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Collection {
    pub enabled: bool,
    pub max_response_bytes: usize,
    pub max_items: usize,
    pub per_host: usize,
    pub user_agent: String,
}

impl Default for Collection {
    fn default() -> Self {
        Self {
            enabled: false,
            max_response_bytes: 2_097_152,
            max_items: 200,
            per_host: 2,
            user_agent: "BreakingBytes/0.1".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pipeline {
    pub max_age_hours: u32,
    pub cluster_similarity: f64,
    pub daily_cap: u32,
    pub breaking_score: f64,
    pub min_gap_seconds: u64,
    pub max_rewrite_attempts: u8,
    pub channel_capacity: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Database {
    pub url: String,
    pub max_connections: u32,
    pub timeout_s: u64,
    pub lease_s: u64,
    pub max_attempts: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admin {
    pub bind: SocketAddr,
    pub token: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instagram {
    pub user_id: String,
    pub access_token: String,
    pub api_version: String,
    pub login: Login,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Login {
    Facebook,
    Instagram,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Storage {
    pub bucket: String,
    pub ttl_minutes: u32,
    #[serde(default = "r2_account")]
    pub account_id: String,
    #[serde(default = "r2_access")]
    pub access_key: String,
    #[serde(default = "r2_secret")]
    pub secret_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Llm {
    #[serde(default)]
    pub enabled: bool,
    pub model: String,
    pub timeout_s: u64,
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    #[serde(default)]
    pub origin_group: Option<String>,
    pub name: String,
    pub layer: String,
    pub kind: String,
    pub url: String,
    pub weight: u8,
    pub interval_s: u64,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn r2_account() -> String {
    "env:R2_ACCOUNT_ID".into()
}
fn r2_access() -> String {
    "env:R2_ACCESS_KEY_ID".into()
}
fn r2_secret() -> String {
    "env:R2_SECRET_ACCESS_KEY".into()
}

fn enabled_by_default() -> bool {
    true
}

/// O valor só é exposto explicitamente no ponto de uso da credencial.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[SEGREDO OCULTO]")
    }
}

#[derive(Debug)]
pub struct Secrets {
    pub database_url: Secret,
    pub admin_token: Secret,
    pub instagram_token: Option<Secret>,
    pub instagram_user_id: Option<Secret>,
    pub llm_key: Option<Secret>,
    pub r2_account: Option<Secret>,
    pub r2_access: Option<Secret>,
    pub r2_secret: Option<Secret>,
}

impl Settings {
    /// Executar antes de construir o runtime; leitura limitada a 1 MiB por arquivo.
    pub fn load(config: &Path, sources: &Path) -> Result<Self, ConfigError> {
        for path in [config, sources] {
            let metadata = std::fs::metadata(path).map_err(|_| ConfigError::Read)?;
            if metadata.len() > 1_048_576 {
                return Err(ConfigError::TooLarge);
            }
        }
        let mut settings = Self::from_figment(
            Figment::new()
                .merge(Toml::file(config))
                .merge(Toml::file(sources))
                .merge(Env::prefixed("STORIES__").split("__")),
        )?;
        if Path::new(&settings.imaging.manifest).is_relative() {
            let base = config
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            settings.imaging.manifest = base
                .join(&settings.imaging.manifest)
                .to_str()
                .ok_or(ConfigError::Invalid("caminho do manifesto de imagens"))?
                .to_owned();
        }
        Ok(settings)
    }

    pub fn from_figment(figment: Figment) -> Result<Self, ConfigError> {
        let settings: Self = figment.extract().map_err(|_| ConfigError::Decode)?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.publishing.reserved_slots > 79 {
            return Err(ConfigError::Invalid("publishing.reserved_slots"));
        }
        let image = &self.imaging;
        if !(1024..=8 * 1024 * 1024).contains(&image.max_bytes)
            || !(1_000_000..=16_000_000).contains(&image.max_pixels)
            || image.allowed_domains.len() > 32
            || image.allowed_domains.iter().any(|h| {
                h.is_empty() || h.contains('/') || h.contains('*') || h != &h.to_ascii_lowercase()
            })
            || [&image.manifest, &image.artifacts_dir].iter().any(|p| {
                p.is_empty()
                    || p.len() > 1024
                    || p.chars().any(char::is_control)
                    || std::path::Path::new(p)
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
            })
        {
            return Err(ConfigError::Invalid("imaging"));
        }
        let c = &self.collection;
        if !(1024..=8_388_608).contains(&c.max_response_bytes)
            || !(1..=200).contains(&c.max_items)
            || !(1..=4).contains(&c.per_host)
            || !c.user_agent.starts_with("BreakingBytes/")
            || c.user_agent.len() > 256
            || c.user_agent.chars().any(char::is_control)
            || self.sources.len() > 32
        {
            return Err(ConfigError::Invalid("collection"));
        }
        let p = &self.pipeline;
        if !(1..=168).contains(&p.max_age_hours)
            || !(0.0..=1.0).contains(&p.cluster_similarity)
            || !(0.0..=1.0).contains(&p.breaking_score)
            || !(1..=80).contains(&p.daily_cap)
            || !(1..=86_400).contains(&p.min_gap_seconds)
            || p.max_rewrite_attempts > 2
            || !(1..=4096).contains(&p.channel_capacity)
        {
            return Err(ConfigError::Invalid("pipeline"));
        }
        let d = &self.database;
        if !(1..=10).contains(&d.max_connections)
            || !(1..=30).contains(&d.timeout_s)
            || !(30..=3600).contains(&d.lease_s)
            || !(1..=20).contains(&d.max_attempts)
        {
            return Err(ConfigError::Invalid("database"));
        }
        for reference in [
            &d.url,
            &self.admin.token,
            &self.instagram.user_id,
            &self.instagram.access_token,
            &self.llm.api_key,
            &self.storage.account_id,
            &self.storage.access_key,
            &self.storage.secret_key,
        ] {
            secret_name(reference)?;
        }
        if !(1..=60).contains(&self.llm.timeout_s)
            || self.llm.model.trim().is_empty()
            || self.storage.bucket.trim().is_empty()
            || !(30..=1440).contains(&self.storage.ttl_minutes)
            || !valid_api_version(&self.instagram.api_version)
        {
            return Err(ConfigError::Invalid("serviços externos"));
        }
        let mut names = HashSet::new();
        for source in &self.sources {
            if source.name.chars().count() > 80
                || source.name.chars().any(char::is_control)
                || source.origin_group.as_ref().is_some_and(|v| {
                    v.trim().is_empty() || v.len() > 80 || v.chars().any(char::is_control)
                })
            {
                return Err(ConfigError::Invalid("identidade editorial da fonte"));
            }
            let parsed =
                url::Url::parse(&source.url).map_err(|_| ConfigError::Invalid("sources.url"))?;
            if parsed.host_str().is_none()
                || !parsed.username().is_empty()
                || parsed.password().is_some()
            {
                return Err(ConfigError::Invalid("sources.url"));
            }
            if source.enabled && source.kind == "edgar" && !self.collection.user_agent.contains('@')
            {
                return Err(ConfigError::Invalid("SEC exige User-Agent com contato"));
            }
            if source.name.trim().is_empty()
                || !names.insert(&source.name)
                || !matches!(source.layer.as_str(), "A" | "B" | "C" | "D")
                || !(1..=5).contains(&source.weight)
                || !(15..=86_400).contains(&source.interval_s)
                || !matches!(
                    source.kind.as_str(),
                    "rss" | "sitemap" | "github" | "edgar" | "hackernews" | "status"
                )
                || !(source.url.starts_with("https://") || source.url.starts_with("http://"))
            {
                return Err(ConfigError::Invalid("sources"));
            }
        }
        Ok(())
    }

    pub fn resolve_secrets(&self) -> Result<Secrets, ConfigError> {
        self.resolve_with(|name| std::env::var(name).ok())
    }

    /// Injeção da consulta evita modificar o ambiente global durante os testes.
    pub fn resolve_with(
        &self,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Secrets, ConfigError> {
        let required = |reference: &str| -> Result<Secret, ConfigError> {
            let name = secret_name(reference)?;
            lookup(name)
                .filter(|value| !value.trim().is_empty())
                .map(Secret)
                .ok_or_else(|| ConfigError::MissingSecret(name.to_owned()))
        };
        let optional = |reference: &str| -> Result<Option<Secret>, ConfigError> {
            if self.instagram.dry_run || !self.publishing.enabled {
                Ok(lookup(secret_name(reference)?)
                    .filter(|value| !value.trim().is_empty())
                    .map(Secret))
            } else {
                required(reference).map(Some)
            }
        };
        let admin_token = required(&self.admin.token)?;
        if admin_token.expose().len() < 32 {
            return Err(ConfigError::Invalid(
                "admin.token deve ter no mínimo 32 bytes",
            ));
        }
        Ok(Secrets {
            database_url: required(&self.database.url)?,
            admin_token,
            r2_account: optional(&self.storage.account_id)?,
            r2_access: optional(&self.storage.access_key)?,
            r2_secret: optional(&self.storage.secret_key)?,
            instagram_token: optional(&self.instagram.access_token)?,
            instagram_user_id: optional(&self.instagram.user_id)?,
            llm_key: if self.llm.enabled {
                Some(required(&self.llm.api_key)?)
            } else {
                lookup(secret_name(&self.llm.api_key)?)
                    .filter(|v| !v.trim().is_empty())
                    .map(Secret)
            },
        })
    }
}

fn valid_api_version(value: &str) -> bool {
    value
        .strip_prefix('v')
        .and_then(|tail| tail.split_once('.'))
        .is_some_and(|(major, minor)| {
            !major.is_empty()
                && !minor.is_empty()
                && major.bytes().all(|b| b.is_ascii_digit())
                && minor.bytes().all(|b| b.is_ascii_digit())
        })
}

fn secret_name(reference: &str) -> Result<&str, ConfigError> {
    reference
        .strip_prefix("env:")
        .filter(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        })
        .ok_or(ConfigError::SecretReference)
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("não foi possível ler os arquivos de configuração")]
    Read,
    #[error("arquivo de configuração excede 1 MiB")]
    TooLarge,
    #[error("configuração inválida: confira tipos, campos e sintaxe TOML")]
    Decode,
    #[error("configuração fora dos limites: {0}")]
    Invalid(&'static str),
    #[error("segredos devem ser referências env:NOME_DA_VARIAVEL")]
    SecretReference,
    #[error("variável de ambiente obrigatória ausente: {0}")]
    MissingSecret(String),
}

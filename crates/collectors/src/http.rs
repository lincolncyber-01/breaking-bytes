//! Cliente compartilhado: DNS público, redirecionamentos revalidados e respostas limitadas.

use reqwest::{
    Client, Url,
    dns::{Addrs, Name, Resolve, Resolving},
    header,
};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::sync::{Mutex, Semaphore};

#[derive(Debug, Clone, Default)]
pub struct Validators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub validators: Validators,
    pub url: Url,
}

struct HostState {
    semaphore: Arc<Semaphore>,
    robots: Option<(Instant, String)>,
}

#[derive(Clone)]
pub struct HttpClient {
    news_blocked: bool,
    client: Client,
    hosts: Arc<Mutex<HashMap<String, HostState>>>,
    max_bytes: usize,
    per_host: usize,
    robots_cpu: Arc<Semaphore>,
    sec_next: Arc<Mutex<Instant>>,
    #[cfg(test)]
    local_origin: Option<String>,
}

impl HttpClient {
    /// Permite reutilizar o pool em APIs de destino fixo, sem liberar URLs de notícias.
    pub fn shared(user_agent: &str, max_bytes: usize, per_host: usize) -> Result<Self, HttpError> {
        Self::build(user_agent, max_bytes, per_host)
    }
    pub fn trusted_api_client(&self) -> Client {
        self.client.clone()
    }
    pub fn ensure_news_egress(&self) -> Result<(), HttpError> {
        if self.news_blocked {
            Err(HttpError::ProxyConfiguration)
        } else {
            Ok(())
        }
    }
    /// Proxy resolve nomes fora do processo e impossibilita esta garantia de pinning.
    pub fn new(user_agent: &str, max_bytes: usize, per_host: usize) -> Result<Self, HttpError> {
        if [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ]
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()))
        {
            return Err(HttpError::ProxyConfiguration);
        }
        Self::build(user_agent, max_bytes, per_host)
    }

    fn build(user_agent: &str, max_bytes: usize, per_host: usize) -> Result<Self, HttpError> {
        if !(1024..=8_388_608).contains(&max_bytes) || !(1..=4).contains(&per_host) {
            return Err(HttpError::Limits);
        }
        // reqwest 0.13 exige provedor explícito com rustls-no-provider.
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let client = Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(per_host)
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(PublicResolver))
            .build()
            .map_err(|_| HttpError::Network)?;
        Ok(Self {
            news_blocked: [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ]
            .iter()
            .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty())),
            client,
            hosts: Arc::new(Mutex::new(HashMap::new())),
            max_bytes,
            per_host,
            robots_cpu: Arc::new(Semaphore::new(2)),
            sec_next: Arc::new(Mutex::new(Instant::now())),
            #[cfg(test)]
            local_origin: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn local(origin: &str, max_bytes: usize) -> Result<Self, HttpError> {
        let mut value = Self::build("BreakingBytesTests", max_bytes, 2)?;
        value.news_blocked = false;
        value.client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(1))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| HttpError::Network)?;
        value.local_origin = Some(origin.to_owned());
        Ok(value)
    }

    fn validate(&self, url: &Url) -> Result<(), HttpError> {
        #[cfg(test)]
        if self.local_origin.as_deref() == Some(url.origin().ascii_serialization().as_str()) {
            return Ok(());
        }
        validate_url(url)
    }

    async fn semaphore(&self, url: &Url) -> Result<Arc<Semaphore>, HttpError> {
        let mut hosts = self.hosts.lock().await;
        let key = url.origin().ascii_serialization();
        if !hosts.contains_key(&key) {
            if hosts.len() >= 128 {
                return Err(HttpError::HostLimit);
            }
            hosts.insert(
                key.clone(),
                HostState {
                    semaphore: Arc::new(Semaphore::new(self.per_host)),
                    robots: None,
                },
            );
        }
        hosts
            .get(&key)
            .map(|s| s.semaphore.clone())
            .ok_or(HttpError::HostLimit)
    }

    pub async fn get(&self, url: &str, validators: &Validators) -> Result<Response, HttpError> {
        self.ensure_news_egress()?;
        let url = Url::parse(url).map_err(|_| HttpError::Url)?;
        self.validate(&url)?;
        self.robots(&url).await?;
        let response = self.raw(url, validators, true).await?;
        Ok(response)
    }

    async fn robots(&self, url: &Url) -> Result<(), HttpError> {
        let key = url.origin().ascii_serialization();
        self.semaphore(url).await?;
        let cached = self
            .hosts
            .lock()
            .await
            .get(&key)
            .and_then(|h| h.robots.clone())
            .filter(|(at, _)| at.elapsed() < Duration::from_secs(3600));
        let body = if let Some((_, body)) = cached {
            body
        } else {
            let mut robots = url.clone();
            robots.set_path("/robots.txt");
            robots.set_query(None);
            robots.set_fragment(None);
            let response = self.raw(robots, &Validators::default(), false).await?;
            let body = match response.status {
                200 => {
                    if response.body.len() > 65_536 {
                        return Err(HttpError::Limits);
                    }
                    String::from_utf8(response.body).map_err(|_| HttpError::Robots)?
                }
                404 | 410 => String::new(),
                401 | 403 => "User-agent: *\nDisallow: /".into(),
                code => {
                    return Err(HttpError::Status {
                        code,
                        retry_after: None,
                    });
                }
            };
            if let Some(host) = self.hosts.lock().await.get_mut(&key) {
                host.robots = Some((Instant::now(), body.clone()));
            }
            body
        };
        self.robots_allowed(body, url).await
    }

    async fn robots_allowed(&self, body: String, url: &Url) -> Result<(), HttpError> {
        let permit = self
            .robots_cpu
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| HttpError::Robots)?;
        let url = url.to_string();
        let allowed = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            robotstxt::DefaultMatcher::default().one_agent_allowed_by_robots(
                &body,
                "BreakingBytes",
                &url,
            )
        })
        .await
        .map_err(|_| HttpError::Robots)?;
        if allowed {
            Ok(())
        } else {
            Err(HttpError::Robots)
        }
    }

    async fn raw(
        &self,
        mut url: Url,
        validators: &Validators,
        check_redirect_robots: bool,
    ) -> Result<Response, HttpError> {
        tokio::time::timeout(Duration::from_secs(10), async {
            for hop in 0..=3 {
                self.validate(&url)?;
                let semaphore = self.semaphore(&url).await?;
                let _permit = semaphore
                    .acquire()
                    .await
                    .map_err(|_| HttpError::HostLimit)?;
                if url
                    .host_str()
                    .is_some_and(|h| h == "sec.gov" || h.ends_with(".sec.gov"))
                {
                    let slot = {
                        let mut next = self.sec_next.lock().await;
                        let slot = (*next).max(Instant::now());
                        *next = slot + Duration::from_millis(200);
                        slot
                    };
                    tokio::time::sleep_until(tokio::time::Instant::from_std(slot)).await;
                }
                let mut request = self.client.get(url.clone());
                if let Some(value) = &validators.etag {
                    request = request.header(header::IF_NONE_MATCH, value);
                }
                if let Some(value) = &validators.last_modified {
                    request = request.header(header::IF_MODIFIED_SINCE, value);
                }
                let mut response = request.send().await.map_err(|_| HttpError::Network)?;
                let status = response.status().as_u16();
                if matches!(status, 301 | 302 | 303 | 307 | 308) {
                    if hop == 3 {
                        return Err(HttpError::Redirect);
                    }
                    let location = response
                        .headers()
                        .get(header::LOCATION)
                        .and_then(|v| v.to_str().ok())
                        .ok_or(HttpError::Redirect)?;
                    let target = url.join(location).map_err(|_| HttpError::Redirect)?;
                    self.validate(&target)?;
                    // robots.txt redirecionado nunca consulta robots.txt recursivamente.
                    if check_redirect_robots && target.origin() != url.origin() {
                        return Err(HttpError::CrossOriginRedirect);
                    }
                    if check_redirect_robots {
                        let body = self
                            .hosts
                            .lock()
                            .await
                            .get(&target.origin().ascii_serialization())
                            .and_then(|s| s.robots.as_ref())
                            .map(|(_, body)| body.clone())
                            .ok_or(HttpError::Robots)?;
                        self.robots_allowed(body, &target).await?;
                    }
                    url = target;
                    continue;
                }
                let retry_after = response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| {
                        v.parse::<u64>().ok().or_else(|| {
                            chrono::DateTime::parse_from_rfc2822(v).ok().and_then(|d| {
                                u64::try_from(
                                    (d.with_timezone(&chrono::Utc) - chrono::Utc::now())
                                        .num_seconds(),
                                )
                                .ok()
                            })
                        })
                    })
                    .map(|v| v.clamp(1, 86_400));
                if !(200..=299).contains(&status)
                    && status != 304
                    && !matches!(status, 401 | 403 | 404 | 410)
                {
                    return Err(HttpError::Status {
                        code: status,
                        retry_after,
                    });
                }
                let headers = Validators {
                    etag: bounded_header(response.headers(), header::ETAG),
                    last_modified: bounded_header(response.headers(), header::LAST_MODIFIED),
                };
                if response
                    .content_length()
                    .is_some_and(|n| n > self.max_bytes as u64)
                {
                    return Err(HttpError::Limits);
                }
                let mut body = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|_| HttpError::Network)? {
                    if body.len().saturating_add(chunk.len()) > self.max_bytes {
                        return Err(HttpError::Limits);
                    }
                    body.extend_from_slice(&chunk);
                }
                return Ok(Response {
                    status,
                    body,
                    validators: headers,
                    url,
                });
            }
            Err(HttpError::Redirect)
        })
        .await
        .map_err(|_| HttpError::Timeout)?
    }
}

fn bounded_header(headers: &header::HeaderMap, key: header::HeaderName) -> Option<String> {
    headers
        .get(key)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 1024)
        .map(str::to_owned)
}

pub fn validate_url(url: &Url) -> Result<(), HttpError> {
    if url.as_str().len() > 8192
        || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.port_or_known_default(), Some(80 | 443))
    {
        return Err(HttpError::Url);
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) if public_ip(IpAddr::V4(ip)) => Ok(()),
        Some(url::Host::Ipv6(ip)) if public_ip(IpAddr::V6(ip)) => Ok(()),
        Some(url::Host::Domain(host))
            if host.contains('.') && !host.ends_with(".localhost") && !host.ends_with(".local") =>
        {
            Ok(())
        }
        _ => Err(HttpError::Ssrf),
    }
}

pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let value = u32::from(ip);
            let blocked = [
                (0x00000000, 8),
                (0x0a000000, 8),
                (0x64400000, 10),
                (0x7f000000, 8),
                (0xa9fe0000, 16),
                (0xac100000, 12),
                (0xc0000000, 24),
                (0xc0000200, 24),
                (0xc0586300, 24),
                (0xc0a80000, 16),
                (0xc6120000, 15),
                (0xc6336400, 24),
                (0xcb007100, 24),
                (0xe0000000, 4),
                (0xf0000000, 4),
            ];
            !blocked
                .into_iter()
                .any(|(prefix, bits)| value & (u32::MAX << (32 - bits)) == prefix)
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            (0x2000..=0x3fff).contains(&s[0])
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] <= 0x01ff || s[1] == 0x0db8))
                && s[0] != 0x3fff
        }
    }
}

struct PublicResolver;
impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses: Vec<SocketAddr> = tokio::time::timeout(
                Duration::from_secs(3),
                tokio::net::lookup_host((host.as_str(), 0)),
            )
            .await??
            .collect();
            if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
                return Err(Box::new(std::io::Error::other("DNS não público"))
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

#[derive(Debug, Error)]
pub enum HttpError {
    #[error("proxy de ambiente impede a garantia de DNS público; use egress validado")]
    ProxyConfiguration,
    #[error("URL externa inválida")]
    Url,
    #[error("destino bloqueado pela política SSRF")]
    Ssrf,
    #[error("falha de rede ou TLS")]
    Network,
    #[error("requisição externa excedeu o timeout")]
    Timeout,
    #[error("resposta excedeu o limite de bytes")]
    Limits,
    #[error("limite de hosts ou concorrência excedido")]
    HostLimit,
    #[error("redirecionamento inválido ou em excesso")]
    Redirect,
    #[error("redirecionamento para outra origem exige cadastro explícito da fonte")]
    CrossOriginRedirect,
    #[error("robots.txt bloqueou a requisição")]
    Robots,
    #[error("servidor externo retornou HTTP {code}")]
    Status { code: u16, retry_after: Option<u64> },
}

impl HttpError {
    pub fn retry_after(&self) -> Option<u64> {
        if let Self::Status { retry_after, .. } = self {
            *retry_after
        } else {
            None
        }
    }
}

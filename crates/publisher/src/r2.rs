//! SDK usado exclusivamente para SigV4. Todo I/O usa o pool reqwest compartilhado.

use crate::{MAX_JPEG, PublishError, TIMEOUT, body, digest};
use aws_sdk_s3::{
    Client as Signer,
    config::{BehaviorVersion, Credentials, Region, RequestChecksumCalculation},
    presigning::{PresignedRequest, PresigningConfig},
};
use reqwest::{Client, Method};
use std::{fmt, time::Duration};

pub struct SignedUrl(String);
impl SignedUrl {
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for SignedUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[URL ASSINADA OCULTA]")
    }
}

#[derive(Clone)]
pub struct R2 {
    signer: Signer,
    http: Client,
    bucket: String,
    ttl: Duration,
}

impl R2 {
    pub fn new(
        http: Client,
        account_id: &str,
        bucket: &str,
        access_key: &str,
        secret_key: &str,
        ttl: Duration,
    ) -> Result<Self, PublishError> {
        if account_id.len() != 32
            || !account_id.bytes().all(|v| v.is_ascii_hexdigit())
            || bucket.len() < 3
            || bucket.len() > 63
            || !bucket
                .bytes()
                .all(|v| v.is_ascii_lowercase() || v.is_ascii_digit() || v == b'-')
            || access_key.is_empty()
            || secret_key.is_empty()
            || !(1800..=86400).contains(&ttl.as_secs())
        {
            return Err(PublishError::Configuration);
        }
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("auto"))
            .credentials_provider(Credentials::new(
                access_key, secret_key, None, None, "r2-env",
            ))
            .endpoint_url(format!("https://{account_id}.r2.cloudflarestorage.com"))
            .force_path_style(true)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .build();
        // Configuração inteiramente estática; rt-tokio fornece relógio/sleep e não há cadeia de credenciais.
        let signer = Signer::from_conf(config);
        Ok(Self {
            signer,
            http,
            bucket: bucket.into(),
            ttl,
        })
    }
    fn key(key: &str) -> Result<(), PublishError> {
        if key.is_empty()
            || key.len() > 256
            || key.contains("..")
            || !key
                .bytes()
                .all(|v| v.is_ascii_alphanumeric() || matches!(v, b'/' | b'-' | b'.' | b'_'))
        {
            return Err(PublishError::Configuration);
        }
        Ok(())
    }
    fn presign(&self) -> Result<PresigningConfig, PublishError> {
        PresigningConfig::expires_in(self.ttl).map_err(|_| PublishError::Signing)
    }
    async fn send(
        &self,
        method: Method,
        request: PresignedRequest,
        bytes: Option<Vec<u8>>,
    ) -> Result<reqwest::Response, PublishError> {
        let mut builder = self.http.request(method, request.uri()).timeout(TIMEOUT);
        for (name, value) in request.headers() {
            builder = builder.header(name, value);
        }
        if let Some(bytes) = bytes {
            builder = builder.body(bytes);
        }
        builder.send().await.map_err(|_| PublishError::Transport)
    }
    pub async fn get_url(&self, key: &str) -> Result<SignedUrl, PublishError> {
        Self::key(key)?;
        let signed = self
            .signer
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(self.presign()?)
            .await
            .map_err(|_| PublishError::Signing)?;
        Ok(SignedUrl(signed.uri().into()))
    }
    pub async fn put_verified(
        &self,
        key: &str,
        bytes: Vec<u8>,
        hash: [u8; 32],
    ) -> Result<(), PublishError> {
        Self::key(key)?;
        if bytes.is_empty() || bytes.len() > MAX_JPEG {
            return Err(PublishError::Artifact);
        }
        let (bytes, computed) = digest(bytes).await?;
        if computed != hash {
            return Err(PublishError::Artifact);
        }
        let request = self
            .signer
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type("image/jpeg")
            .presigned(self.presign()?)
            .await
            .map_err(|_| PublishError::Signing)?;
        let response = self.send(Method::PUT, request, Some(bytes)).await?;
        if !response.status().is_success() {
            return Err(crate::response_error(&response));
        }
        let url = self.get_url(key).await?;
        tokio::time::timeout(TIMEOUT, async {
            let response = self
                .http
                .get(url.expose())
                .timeout(TIMEOUT)
                .send()
                .await
                .map_err(|_| PublishError::Transport)?;
            if !response.status().is_success() {
                return Err(crate::response_error(&response));
            }
            if digest(body(response, MAX_JPEG).await?).await?.1 != hash {
                return Err(PublishError::Artifact);
            }
            Ok(())
        })
        .await
        .map_err(|_| PublishError::Transport)?
    }
    pub async fn delete(&self, key: &str) -> Result<(), PublishError> {
        Self::key(key)?;
        let request = self
            .signer
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(self.presign()?)
            .await
            .map_err(|_| PublishError::Signing)?;
        let response = self.send(Method::DELETE, request, None).await?;
        if response.status().is_success() || response.status().as_u16() == 404 {
            Ok(())
        } else {
            Err(crate::response_error(&response))
        }
    }
}

#[cfg(test)]
mod tests;

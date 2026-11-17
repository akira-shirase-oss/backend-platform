//! The HTTP adapter: one shared client with connect and request deadlines, no proxies, no
//! redirects (adapters connect only to their configured endpoint, T7), bounded replies.

use std::time::Duration;

use platform_domain::{
    CreateRequest, Observation, ObservedResource, ProviderName, ProviderResourceId, ResourceKind,
};
use reqwest::StatusCode;

use crate::wire::{self, CreateBody, CreatedBody, LookupBody, MAX_REPLY_BYTES};
use crate::{Provider, ProviderError};

/// Where the provider is and how long a call may take.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpProviderConfig {
    /// Base URL, e.g. `http://127.0.0.1:61410`.
    pub base_url: String,
    /// Longest TCP connect.
    pub connect_timeout: Duration,
    /// Longest whole request.
    pub request_timeout: Duration,
}

/// A provider reached over HTTP.
#[derive(Debug, Clone)]
pub struct HttpProvider {
    client: reqwest::Client,
    base: String,
}

impl HttpProvider {
    /// Builds the client.
    pub fn new(config: &HttpProviderConfig) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .pool_max_idle_per_host(64)
            .build()?;
        Ok(Self {
            client,
            base: config.base_url.trim_end_matches('/').to_owned(),
        })
    }
}

fn classify(error: &reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Timeout
    } else if error.is_decode() || error.is_body() {
        ProviderError::Protocol
    } else {
        ProviderError::Unavailable
    }
}

async fn read_bounded(mut response: reqwest::Response) -> Result<Vec<u8>, ProviderError> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_REPLY_BYTES as u64)
    {
        return Err(ProviderError::Protocol);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| classify(&e))? {
        if body.len() + chunk.len() > MAX_REPLY_BYTES {
            return Err(ProviderError::Protocol);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn status_error(status: StatusCode) -> ProviderError {
    match status {
        StatusCode::CONFLICT => ProviderError::AlreadyExists,
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => ProviderError::Rejected,
        s if s.is_server_error() || s == StatusCode::TOO_MANY_REQUESTS => {
            ProviderError::Unavailable
        }
        _ => ProviderError::Protocol,
    }
}

impl Provider for HttpProvider {
    async fn lookup(
        &self,
        kind: ResourceKind,
        name: &ProviderName,
    ) -> Result<Observation, ProviderError> {
        let url = format!("{}{}", self.base, wire::lookup_path(kind, name));
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| classify(&e))?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(Observation::Absent),
            StatusCode::OK => {
                let body = read_bounded(response).await?;
                let found: LookupBody =
                    serde_json::from_slice(&body).map_err(|_| ProviderError::Protocol)?;
                if found.matches.is_empty() || found.matches.iter().any(|m| m.spec.kind() != kind) {
                    return Err(ProviderError::Protocol);
                }
                Ok(Observation::from_matches(
                    found
                        .matches
                        .into_iter()
                        .map(|m| ObservedResource {
                            id: m.id,
                            spec: m.spec,
                            tag: m.tag,
                        })
                        .collect(),
                ))
            }
            status => Err(status_error(status)),
        }
    }

    async fn create(&self, request: &CreateRequest) -> Result<ProviderResourceId, ProviderError> {
        let url = format!("{}{}", self.base, wire::create_path(request.kind));
        let body = CreateBody {
            name: request.name.clone(),
            spec: request.spec.clone(),
            tag: request.tag,
            idempotency_key: request.idempotency_key.clone(),
        };
        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|e| classify(&e))?;
        match response.status() {
            StatusCode::CREATED => {
                let body = read_bounded(response).await?;
                let created: CreatedBody =
                    serde_json::from_slice(&body).map_err(|_| ProviderError::Protocol)?;
                Ok(created.id)
            }
            status => Err(status_error(status)),
        }
    }
}

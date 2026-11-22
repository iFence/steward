//! Scoped HTTPS requests for one isolated plugin host.

use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nsh_core::access::HttpGrant;
use nsh_engine::HostReply;
use steward_plugin_contract::capabilities::NET_FETCH;
use steward_plugin_contract::host_api::{HostCallError, HostErrorCode};
use steward_plugin_host::PluginAuthorization;
use url::Url;

use crate::host_errors::RejectHostError;

const NETWORK_WORKERS: usize = 2;
const NETWORK_QUEUE_DEPTH: usize = 8;
const MAX_REDIRECTS: usize = 5;
const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;
const DEFAULT_TIMEOUT_MS: u64 = 10_000;
const MAX_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_HEADERS: usize = 32;

pub(crate) type NetworkAudit = Arc<dyn Fn(&str, &str, &str, &str) + Send + Sync>;

#[derive(Debug, Clone)]
pub(crate) struct NetworkRequest {
    pub(crate) url: String,
    pub(crate) method: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Option<String>,
    pub(crate) timeout_ms: u64,
    pub(crate) max_response_bytes: u64,
}

impl NetworkRequest {
    pub(crate) fn from_value(value: &serde_json::Value) -> Result<Self, NetworkError> {
        let url = value
            .get("url")
            .and_then(serde_json::Value::as_str)
            .filter(|url| !url.is_empty() && url.len() <= 8 * 1024)
            .ok_or_else(|| NetworkError::invalid("fetch requires a bounded url string"))?;
        let method = value
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("GET")
            .to_ascii_uppercase();
        if !matches!(
            method.as_str(),
            "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE"
        ) {
            return Err(NetworkError::invalid(
                "fetch method must be GET, HEAD, POST, PUT, PATCH, or DELETE",
            ));
        }
        let headers = value
            .get("headers")
            .and_then(serde_json::Value::as_object)
            .map(|headers| {
                if headers.len() > MAX_HEADERS {
                    return Err(NetworkError::invalid("fetch has too many headers"));
                }
                headers
                    .iter()
                    .map(|(name, value)| {
                        let value = value.as_str().ok_or_else(|| {
                            NetworkError::invalid("fetch header values must be strings")
                        })?;
                        validate_header(name, value)?;
                        Ok((name.clone(), value.to_owned()))
                    })
                    .collect()
            })
            .transpose()?
            .unwrap_or_default();
        let body = value
            .get("body")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        if body
            .as_ref()
            .is_some_and(|body| body.len() > MAX_REQUEST_BODY_BYTES)
        {
            return Err(NetworkError::resource(
                "fetch request body exceeds the 1 MiB limit",
            ));
        }
        if matches!(method.as_str(), "GET" | "HEAD") && body.is_some() {
            return Err(NetworkError::invalid(
                "GET and HEAD fetch requests cannot include a body",
            ));
        }
        let timeout_ms = value
            .get("timeoutMs")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_MS);
        if !(100..=MAX_TIMEOUT_MS).contains(&timeout_ms) {
            return Err(NetworkError::invalid(format!(
                "fetch timeoutMs must be between 100 and {MAX_TIMEOUT_MS}"
            )));
        }
        let max_response_bytes = value
            .get("maxResponseBytes")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(DEFAULT_RESPONSE_BYTES);
        if !(1..=MAX_RESPONSE_BYTES).contains(&max_response_bytes) {
            return Err(NetworkError::invalid(format!(
                "fetch maxResponseBytes must be between 1 and {MAX_RESPONSE_BYTES}"
            )));
        }
        Ok(Self {
            url: url.to_owned(),
            method,
            headers,
            body,
            timeout_ms,
            max_response_bytes,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NetworkError {
    code: HostErrorCode,
    message: String,
}

impl NetworkError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: HostErrorCode::InvalidInput,
            message: message.into(),
        }
    }

    fn denied(message: impl Into<String>) -> Self {
        Self {
            code: HostErrorCode::PermissionDenied,
            message: message.into(),
        }
    }

    fn timeout(message: impl Into<String>) -> Self {
        Self {
            code: HostErrorCode::Timeout,
            message: message.into(),
        }
    }

    fn resource(message: impl Into<String>) -> Self {
        Self {
            code: HostErrorCode::ResourceExhausted,
            message: message.into(),
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            code: HostErrorCode::HostFailure,
            message: message.into(),
        }
    }

    pub(crate) fn host_message(&self) -> String {
        format!("[{}] {}", self.code.as_str(), self.message)
    }

    pub(crate) fn into_host_error(self) -> HostCallError {
        HostCallError::new(self.code, self.message)
    }
}

struct NetworkTask {
    request: NetworkRequest,
    reply: HostReply,
}

#[derive(Clone)]
pub(crate) struct NetworkService {
    tasks: crossbeam_channel::Sender<NetworkTask>,
}

impl NetworkService {
    pub(crate) fn new(
        data_dir: std::path::PathBuf,
        authorization: PluginAuthorization,
        audit: NetworkAudit,
    ) -> Self {
        let (tasks, receiver) = crossbeam_channel::bounded::<NetworkTask>(NETWORK_QUEUE_DEPTH);
        for worker in 0..NETWORK_WORKERS {
            let receiver = receiver.clone();
            let data_dir = data_dir.clone();
            let authorization = authorization.clone();
            let audit = audit.clone();
            std::thread::Builder::new()
                .name(format!("steward-plugin-network-{worker}"))
                .spawn(move || {
                    while let Ok(task) = receiver.recv() {
                        let scope = audit_scope(&task.request);
                        let result = execute(&data_dir, &authorization, task.request);
                        match result {
                            Ok(response) => {
                                audit(NET_FETCH, &scope, "allowed", "origin=host_call");
                                task.reply
                                    .resolve(serde_json::to_value(response).unwrap_or_default());
                            }
                            Err(error) => {
                                let outcome = if error.code == HostErrorCode::PermissionDenied {
                                    "denied"
                                } else {
                                    "failed"
                                };
                                audit(NET_FETCH, &scope, outcome, &error.host_message());
                                task.reply.reject_host_error(error.into_host_error());
                            }
                        }
                    }
                })
                .expect("spawn plugin network worker");
        }
        Self { tasks }
    }

    pub(crate) fn enqueue(&self, request: NetworkRequest, reply: HostReply) {
        let task = NetworkTask { request, reply };
        match self.tasks.try_send(task) {
            Ok(()) => {}
            Err(crossbeam_channel::TrySendError::Full(task))
            | Err(crossbeam_channel::TrySendError::Disconnected(task)) => {
                task.reply.reject_code(
                    HostErrorCode::ResourceExhausted,
                    "the network capability queue is full; retry later",
                );
            }
        }
    }
}

fn execute(
    data_dir: &std::path::Path,
    authorization: &PluginAuthorization,
    request: NetworkRequest,
) -> Result<NetworkResponse, NetworkError> {
    let grants = authorized_http_grants(data_dir, authorization)?;
    let started = Instant::now();
    let mut url = Url::parse(&request.url)
        .map_err(|error| NetworkError::invalid(format!("invalid fetch URL: {error}")))?;
    let mut method = request.method.clone();
    authorize_url(&url, &method, &grants)?;

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_millis(request.timeout_ms)))
        .build()
        .into();
    let mut headers = request.headers;
    let mut body = request.body;
    let mut redirects = 0usize;

    loop {
        let mut builder = ureq::http::Request::builder()
            .method(method.as_str())
            .uri(url.as_str());
        for (name, value) in &headers {
            builder = builder.header(name, value);
        }
        let request_value = builder
            .body(())
            .map_err(|error| NetworkError::invalid(format!("build fetch request: {error}")))?;
        let mut response = if let Some(body) = &body {
            agent
                .run(request_value.map(|_| body.as_bytes().to_vec()))
                .map_err(map_ureq_error)?
        } else {
            agent.run(request_value).map_err(map_ureq_error)?
        };

        let status = response.status().as_u16();
        if is_redirect(status) {
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| NetworkError::failed("redirect response has no valid location"))?;
            if redirects >= MAX_REDIRECTS {
                return Err(NetworkError::resource(
                    "fetch exceeded the five-redirect limit",
                ));
            }
            let next = url
                .join(location)
                .map_err(|error| NetworkError::failed(format!("invalid redirect URL: {error}")))?;
            if next.origin() != url.origin() {
                headers.retain(|(name, _)| {
                    !matches!(
                        name.to_ascii_lowercase().as_str(),
                        "authorization" | "cookie" | "proxy-authorization"
                    )
                });
            }
            if status == 303 || ((status == 301 || status == 302) && method == "POST") {
                body = None;
                method = "GET".into();
                headers.retain(|(name, _)| !name.eq_ignore_ascii_case("content-type"));
            }
            authorize_url(&next, &method, &grants)?;
            url = next;
            redirects += 1;
            continue;
        }

        if response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|length| length > request.max_response_bytes)
        {
            return Err(NetworkError::resource(
                "fetch response exceeds the configured size limit",
            ));
        }
        let response_headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value.to_str().ok().map(|value| NetworkHeader {
                    name: name.as_str().to_owned(),
                    value: value.to_owned(),
                })
            })
            .collect();
        let mut response_body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(request.max_response_bytes.saturating_add(1))
            .read_to_end(&mut response_body)
            .map_err(|error| NetworkError::failed(format!("read fetch response: {error}")))?;
        if response_body.len() as u64 > request.max_response_bytes {
            return Err(NetworkError::resource(
                "fetch response exceeds the configured size limit",
            ));
        }
        return Ok(NetworkResponse {
            status,
            url: url.to_string(),
            headers: response_headers,
            body: String::from_utf8_lossy(&response_body).into_owned(),
            redirects,
            elapsed_ms: started.elapsed().as_millis() as u64,
        });
    }
}

fn authorized_http_grants(
    data_dir: &std::path::Path,
    authorization: &PluginAuthorization,
) -> Result<Vec<HttpGrant>, NetworkError> {
    let active = authorization
        .active_grants(data_dir)
        .map_err(|error| NetworkError::failed(error.to_string()))?;
    if !active.contains(&(NET_FETCH.into(), "host=steward".into())) {
        return Err(NetworkError::denied(
            "no active steward.net.fetch host permission",
        ));
    }
    let grants = authorization
        .declared_capabilities
        .network
        .http
        .iter()
        .filter(|grant| {
            active.contains(&(NET_FETCH.into(), http_scope(grant)))
                && !grant.methods.is_empty()
                && grant.methods.iter().all(|method| {
                    matches!(
                        method.as_str(),
                        "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE"
                    )
                })
                && grant.schemes.iter().any(|scheme| scheme == "https")
                && grant.schemes.iter().all(|scheme| scheme == "https")
                && grant.port.is_none_or(|port| port == 443)
        })
        .cloned()
        .collect::<Vec<_>>();
    if grants.is_empty() {
        return Err(NetworkError::denied(
            "no active scoped HTTPS grant matches this plugin",
        ));
    }
    Ok(grants)
}

fn authorize_url(url: &Url, method: &str, grants: &[HttpGrant]) -> Result<(), NetworkError> {
    if url.scheme() != "https" {
        return Err(NetworkError::denied("plugin network access requires HTTPS"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(NetworkError::invalid(
            "fetch URLs cannot contain embedded credentials",
        ));
    }
    let Some(host) = url.host_str() else {
        return Err(NetworkError::invalid("fetch URL has no host"));
    };
    if local_network_host(host) {
        return Err(NetworkError::denied(
            "local-network and IP-literal HTTPS requests are not available",
        ));
    }
    if grants.iter().any(|grant| grant_allows(grant, method, url)) {
        Ok(())
    } else {
        Err(NetworkError::denied(format!(
            "no active fetch grant permits {method} https://{host}{}",
            url.path()
        )))
    }
}

fn grant_allows(grant: &HttpGrant, method: &str, url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let port = url.port_or_known_default().unwrap_or_default();
    let path = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    host_pattern_matches(grant.host.as_str(), host)
        && (grant.methods.is_empty() || grant.methods.iter().any(|allowed| allowed == method))
        && (grant.schemes.is_empty() || grant.schemes.iter().any(|allowed| allowed == "https"))
        && grant.port.is_none_or(|allowed| allowed == port)
        && ((grant.paths.is_empty() && grant.path_prefixes.is_empty())
            || grant.paths.iter().any(|allowed| allowed == path)
            || grant.path_prefixes.iter().any(|prefix| {
                path.strip_prefix(prefix.as_str()).is_some_and(|tail| {
                    tail.is_empty() || prefix.ends_with('/') || tail.starts_with('/')
                })
            }))
}

fn host_pattern_matches(pattern: &str, host: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        return host.len() > suffix.len()
            && host.ends_with(suffix)
            && host.as_bytes()[host.len() - suffix.len() - 1] == b'.';
    }
    pattern == host
}

fn local_network_host(host: &str) -> bool {
    let host = host.trim_matches('.').to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.parse::<std::net::IpAddr>().is_ok()
}

fn validate_header(name: &str, value: &str) -> Result<(), NetworkError> {
    let lower = name.to_ascii_lowercase();
    if name.len() > 128
        || value.len() > 8 * 1024
        || name.is_empty()
        || name
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'-' | b'_' | b'.'))
        || value
            .chars()
            .any(|character| matches!(character, '\r' | '\n' | '\0'))
        || matches!(
            lower.as_str(),
            "host"
                | "content-length"
                | "transfer-encoding"
                | "connection"
                | "upgrade"
                | "proxy-authorization"
                | "proxy-connection"
        )
    {
        return Err(NetworkError::invalid(format!(
            "fetch header '{name}' is invalid or reserved"
        )));
    }
    Ok(())
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn map_ureq_error(error: ureq::Error) -> NetworkError {
    let message = error.to_string();
    if message.to_ascii_lowercase().contains("timed out")
        || message.to_ascii_lowercase().contains("timeout")
    {
        NetworkError::timeout(message)
    } else if message.to_ascii_lowercase().contains("too many redirects") {
        NetworkError::resource(message)
    } else {
        NetworkError::failed(format!("HTTPS request failed: {message}"))
    }
}

fn http_scope(grant: &HttpGrant) -> String {
    let list = |values: &[String]| {
        let mut values = values.to_vec();
        values.sort();
        values.dedup();
        values.join(",")
    };
    let port = grant.port.map(|port| port.to_string()).unwrap_or_default();
    format!(
        "host={};schemes={};methods={};paths={};pathPrefixes={};port={port}",
        grant.host.as_str(),
        list(&grant.schemes),
        list(&grant.methods),
        list(&grant.paths),
        list(&grant.path_prefixes)
    )
}

fn audit_scope(request: &NetworkRequest) -> String {
    let parsed = Url::parse(&request.url).ok();
    let host = parsed.as_ref().and_then(Url::host_str).unwrap_or("invalid");
    let path = parsed.as_ref().map(Url::path).unwrap_or("/");
    format!("host={host};method={};path={path}", request.method)
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct NetworkHeader {
    name: String,
    value: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct NetworkResponse {
    status: u16,
    url: String,
    headers: Vec<NetworkHeader>,
    body: String,
    redirects: usize,
    elapsed_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_https_grants_match_methods_hosts_and_path_segments() {
        let capabilities = nsh_core::access::Capabilities::from_json(&serde_json::json!({
            "network": {
                "http": [{
                    "host": "*.example.com",
                    "methods": ["GET"],
                    "schemes": ["https"],
                    "pathPrefixes": ["/v1"]
                }]
            }
        }))
        .unwrap();
        let grant = &capabilities.network.http[0];
        assert!(grant_allows(
            grant,
            "GET",
            &Url::parse("https://api.example.com/v1/items").unwrap()
        ));
        assert!(!grant_allows(
            grant,
            "GET",
            &Url::parse("https://api.example.com/v10").unwrap()
        ));
        assert!(!grant_allows(
            grant,
            "POST",
            &Url::parse("https://api.example.com/v1/items").unwrap()
        ));
        assert!(!grant_allows(
            grant,
            "GET",
            &Url::parse("https://example.com/v1/items").unwrap()
        ));
    }

    #[test]
    fn local_network_targets_and_reserved_headers_are_rejected() {
        assert!(local_network_host("127.0.0.1"));
        assert!(local_network_host("printer.local"));
        assert!(!local_network_host("api.example.com"));
        assert!(validate_header("Authorization", "Bearer token").is_ok());
        assert!(validate_header("Host", "evil.example").is_err());
        assert!(validate_header("X-Test", "line\r\nInjected: yes").is_err());
    }

    #[test]
    fn request_limits_are_validated_before_io() {
        let request = NetworkRequest::from_value(&serde_json::json!({
            "url": "https://example.com",
            "timeoutMs": 0
        }));
        assert!(request.unwrap_err().message.contains("timeoutMs"));
        let request = NetworkRequest::from_value(&serde_json::json!({
            "url": "https://example.com",
            "method": "TRACE"
        }));
        assert!(request.unwrap_err().message.contains("method"));
    }
}

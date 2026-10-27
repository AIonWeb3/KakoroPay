//! # Pay and Call Tool
//!
//! MCP tool that:
//! 1. Supports legacy registry-based vendor invocations.
//! 2. Supports x402-compliant payments: fetches an HTTPS resource, handles 402 Payment Required
//!    by authorizing token payments through the TypeScript payer worker (or stub), retries
//!    once with the payment header, and decodes the settlement transaction hash.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::time::Duration;
use url::Url;

use crate::account_state;
use crate::policy::{PolicyOutcome, evaluate_call};
use crate::soroban_client::{self, SorobanConfig};

pub const DEFAULT_MAX_PER_CALL: i128 = 10_000_000; // 1 XLM in stroops
pub const DEFAULT_PAYER_TIMEOUT_MS: u64 = 30_000;
pub const MAX_RESPONSE_BODY_SIZE: usize = 2 * 1024 * 1024; // 2 MB size cap
pub const MAX_HEADER_SIZE: usize = 8192; // 8 KB

// ---------------------------------------------------------------------------
// Typed Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayAndCallError {
    InvalidUrl(String),
    BlockedTarget(String),
    PaymentRequirementsInvalid(String),
    NetworkMismatch(String),
    OverCeiling(String),
    PolicyDenied(String),
    BudgetExceeded { required: i128, available: i128 },
    PayerFailed(String),
    PayerTimeout(String),
    FacilitatorRejected(String),
    SettlementMissing(String),

    // Legacy variants preserved for backward compatibility
    InsufficientBudget { required: i128, available: i128 },
    ResourceCallFailed(String),
    ResourceNotFound(String),
    TransientError(String),
    AuthFailed(String),
}

impl fmt::Display for PayAndCallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(msg) => write!(f, "Invalid URL: {msg}"),
            Self::BlockedTarget(msg) => write!(f, "Blocked target: {msg}"),
            Self::PaymentRequirementsInvalid(msg) => {
                write!(f, "Payment requirements invalid: {msg}")
            }
            Self::NetworkMismatch(msg) => write!(f, "Network mismatch: {msg}"),
            Self::OverCeiling(msg) => write!(f, "Amount over ceiling: {msg}"),
            Self::PolicyDenied(reason) => write!(f, "Policy denied: {reason}"),
            Self::BudgetExceeded {
                required,
                available,
            } => {
                write!(
                    f,
                    "Budget exceeded: required {required} stroops, available {available} stroops"
                )
            }
            Self::PayerFailed(msg) => write!(f, "Payer failed: {msg}"),
            Self::PayerTimeout(msg) => write!(f, "Payer timeout: {msg}"),
            Self::FacilitatorRejected(msg) => write!(f, "Facilitator rejected: {msg}"),
            Self::SettlementMissing(msg) => write!(f, "Settlement missing: {msg}"),

            Self::InsufficientBudget {
                required,
                available,
            } => {
                write!(
                    f,
                    "Insufficient budget: required {required} stroops, available {available} stroops"
                )
            }
            Self::ResourceCallFailed(reason) => write!(f, "Resource call failed: {reason}"),
            Self::ResourceNotFound(id) => write!(f, "Resource not found: {id}"),
            Self::TransientError(msg) => write!(f, "Transient error (retryable): {msg}"),
            Self::AuthFailed(msg) => write!(f, "Authorization failed: {msg}"),
        }
    }
}

impl std::error::Error for PayAndCallError {}

impl PayAndCallError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidUrl(_) => "InvalidUrl",
            Self::BlockedTarget(_) => "BlockedTarget",
            Self::PaymentRequirementsInvalid(_) => "PaymentRequirementsInvalid",
            Self::NetworkMismatch(_) => "NetworkMismatch",
            Self::OverCeiling(_) => "OverCeiling",
            Self::PolicyDenied(_) => "PolicyDenied",
            Self::BudgetExceeded { .. } => "BudgetExceeded",
            Self::PayerFailed(_) => "PayerFailed",
            Self::PayerTimeout(_) => "PayerTimeout",
            Self::FacilitatorRejected(_) => "FacilitatorRejected",
            Self::SettlementMissing(_) => "SettlementMissing",
            Self::InsufficientBudget { .. } => "InsufficientBudget",
            Self::ResourceCallFailed(_) => "ResourceCallFailed",
            Self::ResourceNotFound(_) => "ResourceNotFound",
            Self::TransientError(_) => "TransientError",
            Self::AuthFailed(_) => "AuthFailed",
        }
    }

    pub fn to_structured_json(&self) -> String {
        let value = match self {
            Self::BudgetExceeded {
                required,
                available,
            } => serde_json::json!({
                "error": {
                    "code": "BudgetExceeded",
                    "message": self.to_string(),
                    "required": required,
                    "available": available
                }
            }),
            Self::InsufficientBudget {
                required,
                available,
            } => serde_json::json!({
                "error": {
                    "code": "InsufficientBudget",
                    "message": self.to_string(),
                    "required": required,
                    "available": available
                }
            }),
            _ => serde_json::json!({
                "error": {
                    "code": self.code(),
                    "message": self.to_string(),
                }
            }),
        };
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| self.to_string())
    }
}

pub const MAX_TRANSIENT_ATTEMPTS: u32 = 3;

pub fn retry_transient<T, F>(mut op: F) -> Result<T, PayAndCallError>
where
    F: FnMut() -> Result<T, PayAndCallError>,
{
    let mut last_transient = None;
    for _ in 0..MAX_TRANSIENT_ATTEMPTS {
        match op() {
            Ok(value) => return Ok(value),
            Err(PayAndCallError::TransientError(msg)) => {
                last_transient = Some(PayAndCallError::TransientError(msg));
            }
            Err(other) => return Err(other),
        }
    }
    Err(last_transient.unwrap_or_else(|| PayAndCallError::TransientError("retry exhausted".into())))
}

// ---------------------------------------------------------------------------
// Result Types
// ---------------------------------------------------------------------------

/// Response from pay_and_call (both URL x402 calls and legacy registry calls).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayAndCallResult {
    /// The settlement transaction hash.
    pub tx_hash: String,
    /// The ledger sequence where confirmed (0 for off-chain / local).
    pub ledger: u32,
    /// The amount spent in stroops.
    pub amount_spent: i128,
    /// The raw response from the underlying resource.
    pub resource_response: String,

    /// HTTP status code.
    pub status: u16,
    /// Size-capped body of the response.
    pub body: String,
    /// Amount spent in stroops.
    pub amount: i128,
    /// Token asset contract address.
    pub asset: String,
    /// Payee recipient address.
    pub pay_to: String,
}

// ---------------------------------------------------------------------------
// Payment Requirements
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaymentRequirements {
    #[serde(default)]
    pub scheme: String,
    pub network: String,
    pub asset: String,
    #[serde(alias = "payTo", alias = "pay_to")]
    pub pay_to: String,
    pub amount: String,
    #[serde(default)]
    pub extra: Option<serde_json::Value>,
    #[serde(default, alias = "maxTimeoutSeconds", alias = "max_timeout_seconds")]
    pub max_timeout_seconds: Option<u32>,
}

// ---------------------------------------------------------------------------
// Payer Worker JSON Interface
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct WorkerInput<'a> {
    resource_url: &'a str,
    payment_requirements: &'a PaymentRequirements,
}

#[derive(Debug, Deserialize)]
struct WorkerSuccessPayload {
    ok: Option<bool>,
    code: Option<String>,
    message: Option<String>,
    header_name: Option<String>,
    header_value: Option<String>,
    payment_payload: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Security / SSRF Validation
// ---------------------------------------------------------------------------

pub fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    // 0.0.0.0/8 (Broadcast/unspecified)
    if octets[0] == 0 {
        return true;
    }
    // 10.0.0.0/8 (Private)
    if octets[0] == 10 {
        return true;
    }
    // 100.64.0.0/10 (CGNAT)
    if octets[0] == 100 && (octets[1] & 0xC0) == 64 {
        return true;
    }
    // 127.0.0.0/8 (Loopback)
    if ip.is_loopback() {
        return true;
    }
    // 169.254.0.0/16 (Link-local, includes cloud metadata 169.254.169.254)
    if ip.is_link_local() {
        return true;
    }
    // 172.16.0.0/12 (Private)
    if octets[0] == 172 && (octets[1] >= 16 && octets[1] <= 31) {
        return true;
    }
    // 192.168.0.0/16 (Private)
    if octets[0] == 192 && octets[1] == 168 {
        return true;
    }
    // 198.18.0.0/15 (Benchmarking)
    if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) {
        return true;
    }
    // 240.0.0.0/4 (Reserved)
    if (octets[0] & 0xF0) == 240 {
        return true;
    }
    // 255.255.255.255 (Broadcast)
    if ip.is_broadcast() {
        return true;
    }
    false
}

pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => is_blocked_ipv4(ipv4),
        IpAddr::V6(ipv6) => {
            if ipv6.is_loopback() || ipv6.is_unspecified() {
                return true;
            }
            if let Some(mapped) = ipv6.to_ipv4_mapped() {
                return is_blocked_ipv4(mapped);
            }
            let segs = ipv6.segments();
            // IPv4-compatible IPv6 (::a.b.c.d)
            if segs[0] == 0
                && segs[1] == 0
                && segs[2] == 0
                && segs[3] == 0
                && segs[4] == 0
                && segs[5] == 0
            {
                let ipv4 = Ipv4Addr::new(
                    (segs[6] >> 8) as u8,
                    (segs[6] & 0xFF) as u8,
                    (segs[7] >> 8) as u8,
                    (segs[7] & 0xFF) as u8,
                );
                return is_blocked_ipv4(ipv4);
            }
            // Unique local fc00::/7
            if (segs[0] & 0xFE00) == 0xFC00 {
                return true;
            }
            // Link-local fe80::/10
            if (segs[0] & 0xFFC0) == 0xFE80 {
                return true;
            }
            // Discard prefix 100::/64
            if segs[0] == 0x0100 && segs[1] == 0 && segs[2] == 0 && segs[3] == 0 {
                return true;
            }
            false
        }
    }
}

pub fn validate_url_and_check_ssrf(
    url_str: &str,
    allow_test_loopback: bool,
) -> Result<Url, PayAndCallError> {
    let parsed = Url::parse(url_str)
        .map_err(|e| PayAndCallError::InvalidUrl(format!("Malformed URL: {e}")))?;

    let is_https = parsed.scheme() == "https";
    let is_test_http = allow_test_loopback && parsed.scheme() == "http";

    if !is_https && !is_test_http {
        return Err(PayAndCallError::InvalidUrl(format!(
            "Only HTTPS URLs are permitted, got scheme '{}'",
            parsed.scheme()
        )));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| PayAndCallError::InvalidUrl("Missing host in URL".into()))?;

    // Check optional ALLOWED_HOSTS allowlist
    if let Ok(allowed_env) = std::env::var("ALLOWED_HOSTS") {
        let allowed: Vec<String> = allowed_env
            .split(',')
            .map(|h| h.trim().to_lowercase())
            .filter(|h| !h.is_empty())
            .collect();
        if !allowed.is_empty() {
            let host_lower = host.to_lowercase();
            if !allowed.contains(&host_lower) {
                return Err(PayAndCallError::BlockedTarget(format!(
                    "Host '{host}' is not in ALLOWED_HOSTS"
                )));
            }
        }
    }

    // Resolve DNS and verify no blocked IP addresses
    let port = parsed.port_or_known_default().unwrap_or(443);
    let socket_addrs = match (host, port).to_socket_addrs() {
        Ok(addrs) => addrs,
        Err(_) => {
            // Unresolvable host or invalid IP literal
            return Err(PayAndCallError::BlockedTarget(format!(
                "Could not resolve host '{host}'"
            )));
        }
    };

    let mut resolved_any = false;
    for addr in socket_addrs {
        resolved_any = true;
        let ip = addr.ip();
        if !allow_test_loopback && is_blocked_ip(ip) {
            return Err(PayAndCallError::BlockedTarget(format!(
                "Host '{host}' resolves to blocked IP address {ip}"
            )));
        }
    }

    if !resolved_any {
        return Err(PayAndCallError::BlockedTarget(format!(
            "Host '{host}' produced no addresses"
        )));
    }

    Ok(parsed)
}

// ---------------------------------------------------------------------------
// Payment Requirements Parsing
// ---------------------------------------------------------------------------

pub fn parse_payment_requirements(
    header_opt: Option<&str>,
    body_str: &str,
) -> Result<PaymentRequirements, PayAndCallError> {
    // 1. Try decoding PAYMENT-REQUIRED header if present
    if let Some(header_val) = header_opt {
        let trimmed = header_val.trim();
        if !trimmed.is_empty() {
            if let Ok(decoded_bytes) = BASE64.decode(trimmed)
                && let Ok(val) = serde_json::from_slice::<serde_json::Value>(&decoded_bytes)
                && let Ok(req) = extract_single_requirement(&val)
            {
                return Ok(req);
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
                && let Ok(req) = extract_single_requirement(&val)
            {
                return Ok(req);
            }
        }
    }

    // 2. Try parsing body JSON
    let body_json: serde_json::Value = serde_json::from_str(body_str).map_err(|e| {
        PayAndCallError::PaymentRequirementsInvalid(format!("Invalid JSON in 402 body: {e}"))
    })?;

    extract_single_requirement(&body_json)
}

fn extract_single_requirement(
    val: &serde_json::Value,
) -> Result<PaymentRequirements, PayAndCallError> {
    // If val is an array of requirements
    if let Some(arr) = val.as_array() {
        if arr.is_empty() {
            return Err(PayAndCallError::PaymentRequirementsInvalid(
                "Empty payment requirements array".into(),
            ));
        }
        if arr.len() > 1 {
            return Err(PayAndCallError::PaymentRequirementsInvalid(
                "Ambiguous payment requirements: multiple options provided".into(),
            ));
        }
        return parse_single_req_object(&arr[0]);
    }

    // If val has `payment_requirements` or `paymentRequirements`
    let inner = val
        .get("payment_requirements")
        .or_else(|| val.get("paymentRequirements"))
        .unwrap_or(val);

    if let Some(arr) = inner.as_array() {
        if arr.is_empty() {
            return Err(PayAndCallError::PaymentRequirementsInvalid(
                "Empty payment requirements array".into(),
            ));
        }
        if arr.len() > 1 {
            return Err(PayAndCallError::PaymentRequirementsInvalid(
                "Ambiguous payment requirements: multiple options provided".into(),
            ));
        }
        return parse_single_req_object(&arr[0]);
    }

    parse_single_req_object(inner)
}

fn parse_single_req_object(
    val: &serde_json::Value,
) -> Result<PaymentRequirements, PayAndCallError> {
    let req: PaymentRequirements = serde_json::from_value(val.clone()).map_err(|e| {
        PayAndCallError::PaymentRequirementsInvalid(format!("Failed to parse requirements: {e}"))
    })?;

    if req.asset.trim().is_empty() {
        return Err(PayAndCallError::PaymentRequirementsInvalid(
            "Missing 'asset' field".into(),
        ));
    }
    if req.pay_to.trim().is_empty() {
        return Err(PayAndCallError::PaymentRequirementsInvalid(
            "Missing 'pay_to' field".into(),
        ));
    }
    if req.network.trim().is_empty() {
        return Err(PayAndCallError::PaymentRequirementsInvalid(
            "Missing 'network' field".into(),
        ));
    }
    let parsed_amount = req.amount.trim().parse::<i128>().map_err(|_| {
        PayAndCallError::PaymentRequirementsInvalid(format!(
            "Amount '{}' is not a valid integer",
            req.amount
        ))
    })?;
    if parsed_amount <= 0 {
        return Err(PayAndCallError::PaymentRequirementsInvalid(
            "Amount must be positive".into(),
        ));
    }

    Ok(req)
}

fn normalize_network(net: &str) -> &str {
    if let Some(stripped) = net.strip_prefix("stellar:") {
        stripped
    } else {
        net
    }
}

// ---------------------------------------------------------------------------
// Worker Execution
// ---------------------------------------------------------------------------

fn split_command(cmd: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut quote_char = ' ';

    let mut chars = cmd.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_quote {
            if ch == quote_char {
                in_quote = false;
            } else if ch == '\\' && chars.peek() == Some(&quote_char) {
                current.push(chars.next().unwrap());
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            in_quote = true;
            quote_char = ch;
        } else if ch.is_whitespace() {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn run_payer_worker(
    url: &str,
    reqs: &PaymentRequirements,
) -> Result<(String, String), PayAndCallError> {
    let payer_cmd = std::env::var("PAYER_CMD").unwrap_or_else(|_| "node payer/dist/cli.js".into());
    let timeout_ms = std::env::var("PAYER_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_PAYER_TIMEOUT_MS);

    let parts = split_command(&payer_cmd);
    if parts.is_empty() {
        return Err(PayAndCallError::PayerFailed("PAYER_CMD is empty".into()));
    }

    let input_json = serde_json::to_string(&WorkerInput {
        resource_url: url,
        payment_requirements: reqs,
    })
    .map_err(|e| PayAndCallError::PayerFailed(e.to_string()))?;

    let mut child = Command::new(&parts[0])
        .args(&parts[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            PayAndCallError::PayerFailed(format!("Failed to spawn worker '{}': {e}", parts[0]))
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input_json.as_bytes());
        let _ = stdin.write_all(b"\n");
    }

    // Wait with timeout
    let start = std::time::Instant::now();
    let mut finished = false;
    let mut _status = None;

    while start.elapsed() < Duration::from_millis(timeout_ms) {
        match child.try_wait() {
            Ok(Some(s)) => {
                finished = true;
                _status = Some(s);
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(PayAndCallError::PayerFailed(format!("Wait error: {e}"))),
        }
    }

    if !finished {
        let _ = child.kill();
        return Err(PayAndCallError::PayerTimeout(format!(
            "Payer worker timed out after {timeout_ms}ms"
        )));
    }

    let mut stdout_str = String::new();
    let mut stderr_str = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut stdout_str);
    }
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut stderr_str);
    }

    let parsed_output: WorkerSuccessPayload = match serde_json::from_str(stdout_str.trim()) {
        Ok(v) => v,
        Err(_) => {
            let combined = format!("stdout: {stdout_str}, stderr: {stderr_str}");
            return Err(PayAndCallError::PayerFailed(format!(
                "Worker produced invalid JSON: {combined}"
            )));
        }
    };

    if parsed_output.ok != Some(true) {
        let code = parsed_output
            .code
            .clone()
            .unwrap_or_else(|| "PayerFailed".into());
        let msg = parsed_output
            .message
            .or_else(|| parsed_output.code.clone())
            .unwrap_or_else(|| "Payer worker rejected request".into());

        return match code.as_str() {
            "PAYEE_NOT_ALLOWLISTED" => Err(PayAndCallError::PolicyDenied(msg)),
            "AMOUNT_EXCEEDS_MAX_PER_CALL" => Err(PayAndCallError::OverCeiling(msg)),
            "NETWORK_MISMATCH" => Err(PayAndCallError::NetworkMismatch(msg)),
            "OverBudget" => Err(PayAndCallError::BudgetExceeded {
                required: 0,
                available: 0,
            }),
            "RateLimited" | "InvalidContext" | "AgentGuardExecutionDenied" => {
                Err(PayAndCallError::PolicyDenied(msg))
            }
            "INVALID_CLI_INPUT" => Err(PayAndCallError::PaymentRequirementsInvalid(msg)),
            _ => Err(PayAndCallError::PayerFailed(msg)),
        };
    }

    // Default header name and value
    let header_name = parsed_output
        .header_name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "X-PAYMENT".into());

    let header_value = if let Some(val) = parsed_output.header_value {
        val
    } else if let Some(payload) = parsed_output.payment_payload {
        let json_str = serde_json::to_string(&payload).unwrap_or_default();
        BASE64.encode(json_str.as_bytes())
    } else {
        return Err(PayAndCallError::PayerFailed(
            "Worker returned ok: true without payment payload or header value".into(),
        ));
    };

    Ok((header_name, header_value))
}

// ---------------------------------------------------------------------------
// Main x402 Payment Flow
// ---------------------------------------------------------------------------

pub fn pay_and_call_url(url_str: &str) -> Result<PayAndCallResult, PayAndCallError> {
    let allow_test = std::env::var("AGENTPAY_ALLOW_TEST_LOOPBACK")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);
    pay_and_call_url_with_config(url_str, &SorobanConfig::from_env(), allow_test)
}

pub fn pay_and_call_url_with_config(
    url_str: &str,
    config: &SorobanConfig,
    allow_test_loopback: bool,
) -> Result<PayAndCallResult, PayAndCallError> {
    // 1. Validate URL & block SSRF targets
    let validated_url = validate_url_and_check_ssrf(url_str, allow_test_loopback)?;

    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| PayAndCallError::TransientError(e.to_string()))?;

    // Initial request
    let resp = client
        .get(validated_url.as_str())
        .send()
        .map_err(|e| PayAndCallError::TransientError(format!("Initial request failed: {e}")))?;

    let status = resp.status().as_u16();

    // If not 402, return free resource without payment
    if status != 402 {
        let mut body_bytes = Vec::new();
        use std::io::Read as _;
        let _ = resp
            .take(MAX_RESPONSE_BODY_SIZE as u64)
            .read_to_end(&mut body_bytes);
        let body = String::from_utf8_lossy(&body_bytes).to_string();

        return Ok(PayAndCallResult {
            tx_hash: String::new(),
            ledger: 0,
            amount_spent: 0,
            resource_response: body.clone(),
            status,
            body,
            amount: 0,
            asset: String::new(),
            pay_to: String::new(),
        });
    }

    // 2. On 402, extract payment requirements
    let payment_required_header = resp
        .headers()
        .get("PAYMENT-REQUIRED")
        .or_else(|| resp.headers().get("Payment-Required"))
        .or_else(|| resp.headers().get("payment-required"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if let Some(ref val) = payment_required_header
        && val.len() > MAX_HEADER_SIZE
    {
        return Err(PayAndCallError::PaymentRequirementsInvalid(
            "Payment-Required header exceeds MAX_HEADER_SIZE".into(),
        ));
    }

    let mut body_bytes = Vec::new();
    let _ = resp
        .take(MAX_RESPONSE_BODY_SIZE as u64)
        .read_to_end(&mut body_bytes);
    let body_str = String::from_utf8_lossy(&body_bytes);

    let reqs = parse_payment_requirements(payment_required_header.as_deref(), &body_str)?;
    let amount: i128 = reqs.amount.parse().map_err(|_| {
        PayAndCallError::PaymentRequirementsInvalid("Invalid amount in requirements".into())
    })?;

    // 3. Precheck before worker is ever called
    // 3a. Network match
    if normalize_network(&reqs.network) != normalize_network(&config.network) {
        return Err(PayAndCallError::NetworkMismatch(format!(
            "Requirement network '{}' does not match configured network '{}'",
            reqs.network, config.network
        )));
    }

    // 3b. Amount ceiling
    let max_per_call = std::env::var("PAYMENT_MAX_PER_CALL")
        .ok()
        .and_then(|s| s.parse::<i128>().ok())
        .unwrap_or(DEFAULT_MAX_PER_CALL);

    if amount <= 0 || amount > max_per_call {
        return Err(PayAndCallError::OverCeiling(format!(
            "Amount {amount} stroops exceeds per-call ceiling of {max_per_call} stroops"
        )));
    }

    // 3c. Payee rule on account (get_payee_rule)
    let rule_id = if config.is_live() {
        let rule_opt = soroban_client::query_payee_rule(config, &reqs.asset, &reqs.pay_to)
            .map_err(|e| {
                PayAndCallError::TransientError(format!("RPC get_payee_rule failed: {e}"))
            })?;
        rule_opt.ok_or_else(|| {
            PayAndCallError::PolicyDenied(format!(
                "Payee ({}, {}) has no matching rule on smart account",
                reqs.asset, reqs.pay_to
            ))
        })?
    } else {
        // When no contract is configured, check AGENTPAY_STATE or ALLOWED_PAYEES / DENIED_PAYEES
        let status = account_state::load_budget();
        if let Some(payees) = &status.allowed_payees {
            let found = payees
                .iter()
                .find(|p| p.asset == reqs.asset && p.pay_to == reqs.pay_to);
            if let Some(p) = found {
                p.rule_id
            } else {
                return Err(PayAndCallError::PolicyDenied(format!(
                    "Payee ({}, {}) not in allowed_payees policy",
                    reqs.asset, reqs.pay_to
                )));
            }
        } else if let Ok(denied) = std::env::var("DENIED_PAYEES") {
            if denied
                .split(',')
                .any(|d| d.trim() == reqs.pay_to || d.trim() == reqs.asset)
            {
                return Err(PayAndCallError::PolicyDenied(format!(
                    "Payee ({}, {}) is explicitly denied",
                    reqs.asset, reqs.pay_to
                )));
            }
            0
        } else {
            0
        }
    };

    // 3d. Budget covers amount
    if config.is_live() {
        let remaining = soroban_client::query_budget(config, rule_id).map_err(|e| {
            PayAndCallError::TransientError(format!("RPC get_remaining_budget failed: {e}"))
        })?;
        if remaining < amount {
            return Err(PayAndCallError::BudgetExceeded {
                required: amount,
                available: remaining,
            });
        }
    } else {
        let status = account_state::load_budget();
        if status.remaining_stroops < amount {
            return Err(PayAndCallError::BudgetExceeded {
                required: amount,
                available: status.remaining_stroops,
            });
        }
    }

    // 4. Spawn payer worker
    let (header_name, header_value) = run_payer_worker(validated_url.as_str(), &reqs)?;

    // 5. Retry request exactly once with payment header
    let retry_resp = client
        .get(validated_url.as_str())
        .header(&header_name, &header_value)
        .send()
        .map_err(|e| PayAndCallError::TransientError(format!("Paid retry failed: {e}")))?;

    let retry_status = retry_resp.status().as_u16();

    // Never loop! If second request is 402, facilitator rejected payment
    if retry_status == 402 {
        return Err(PayAndCallError::FacilitatorRejected(
            "Payment rejected by resource server (second 402 received)".into(),
        ));
    }

    // 6. Decode settlement response header
    let settlement_header = retry_resp
        .headers()
        .get("PAYMENT-RESPONSE")
        .or_else(|| retry_resp.headers().get("Payment-Response"))
        .or_else(|| retry_resp.headers().get("payment-response"))
        .or_else(|| retry_resp.headers().get("X-PAYMENT-RESPONSE"))
        .or_else(|| retry_resp.headers().get("x-payment-response"))
        .and_then(|v| v.to_str().ok());

    let Some(raw_settlement) = settlement_header else {
        if retry_status == 200 {
            return Err(PayAndCallError::SettlementMissing(
                "Response returned 200 but settlement response header was missing".into(),
            ));
        } else {
            return Err(PayAndCallError::ResourceCallFailed(format!(
                "Resource returned HTTP {retry_status}"
            )));
        }
    };

    if raw_settlement.len() > MAX_HEADER_SIZE {
        return Err(PayAndCallError::SettlementMissing(
            "Settlement header exceeds MAX_HEADER_SIZE".into(),
        ));
    }

    let tx_hash = decode_tx_hash_from_settlement(raw_settlement)?;

    let mut paid_body_bytes = Vec::new();
    let _ = retry_resp
        .take(MAX_RESPONSE_BODY_SIZE as u64)
        .read_to_end(&mut paid_body_bytes);
    let paid_body = String::from_utf8_lossy(&paid_body_bytes).to_string();

    // Log structured decision to stderr
    eprintln!(
        r#"{{"event":"payment_decision","decision":"approved","status":{},"amount":{},"asset":"{}","pay_to":"{}","tx_hash":"{}"}}"#,
        retry_status, amount, reqs.asset, reqs.pay_to, tx_hash
    );

    Ok(PayAndCallResult {
        tx_hash,
        ledger: 0,
        amount_spent: amount,
        resource_response: paid_body.clone(),
        status: retry_status,
        body: paid_body,
        amount,
        asset: reqs.asset,
        pay_to: reqs.pay_to,
    })
}

fn decode_tx_hash_from_settlement(raw_header: &str) -> Result<String, PayAndCallError> {
    let trimmed = raw_header.trim();
    // Try base64 decoding
    let json_val = if let Ok(decoded) = BASE64.decode(trimmed) {
        serde_json::from_slice::<serde_json::Value>(&decoded).unwrap_or_else(|_| {
            serde_json::Value::String(String::from_utf8_lossy(&decoded).to_string())
        })
    } else {
        serde_json::from_str::<serde_json::Value>(trimmed)
            .unwrap_or_else(|_| serde_json::Value::String(trimmed.to_string()))
    };

    if let Some(s) = json_val.as_str()
        && !s.trim().is_empty()
    {
        return Ok(s.trim().to_string());
    }

    if let Some(obj) = json_val.as_object() {
        for key in &[
            "tx_hash",
            "txHash",
            "transactionHash",
            "hash",
            "transaction",
        ] {
            if let Some(val) = obj.get(*key).and_then(|v| v.as_str())
                && !val.trim().is_empty()
            {
                return Ok(val.trim().to_string());
            }
        }
    }

    Err(PayAndCallError::SettlementMissing(
        "Could not extract transaction hash from settlement response header".into(),
    ))
}

// ---------------------------------------------------------------------------
// Unified / Legacy Entry Points
// ---------------------------------------------------------------------------

pub fn pay_and_call_unified(
    url: Option<&str>,
    resource_id: Option<&str>,
    params: &str,
) -> Result<PayAndCallResult, PayAndCallError> {
    if let Some(u) = url {
        return pay_and_call_url(u);
    }
    if let Some(id) = resource_id {
        if id.starts_with("http://") || id.starts_with("https://") {
            return pay_and_call_url(id);
        }
        return pay_and_call(id, params);
    }
    Err(PayAndCallError::InvalidUrl(
        "Either 'url' or 'resource_id' must be specified".into(),
    ))
}

/// Default per-vendor window used when no AGENTPAY_STATE is present.
fn default_window() -> (i128, u32, i128, u32) {
    (0, 0, 10_000_000, 10_000)
}

fn local_tx_hash(resource_id: &str, params: &str) -> String {
    let mut n: u64 = 0xcbf29ce484222325;
    for b in resource_id.bytes().chain(params.bytes()) {
        n ^= b as u64;
        n = n.wrapping_mul(0x100000001b3);
    }
    format!("local_{n:016x}")
}

pub fn pay_and_call(resource_id: &str, params: &str) -> Result<PayAndCallResult, PayAndCallError> {
    if resource_id.starts_with("http://") || resource_id.starts_with("https://") {
        return pay_and_call_url(resource_id);
    }
    pay_and_call_with_config(
        resource_id,
        params,
        &SorobanConfig::from_env(),
        default_window(),
    )
}

#[allow(dead_code)]
pub fn pay_and_call_with_window(
    resource_id: &str,
    params: &str,
    window: (i128, u32, i128, u32),
) -> Result<PayAndCallResult, PayAndCallError> {
    pay_and_call_with_config(resource_id, params, &SorobanConfig::from_env(), window)
}

#[allow(dead_code)]
pub fn pay_and_call_with_config(
    resource_id: &str,
    params: &str,
    config: &SorobanConfig,
    window: (i128, u32, i128, u32),
) -> Result<PayAndCallResult, PayAndCallError> {
    let resources = super::discover::load_registry();
    let resource = resources
        .iter()
        .find(|r| r.id == resource_id)
        .ok_or_else(|| PayAndCallError::ResourceNotFound(resource_id.to_string()))?;

    let amount = resource.price as i128;
    if config.is_live()
        && let Ok(remaining) = soroban_client::query_budget(config, config.rule_id)
        && remaining < amount
    {
        return Err(PayAndCallError::InsufficientBudget {
            required: amount,
            available: remaining,
        });
    }

    let (spent, calls, max_spend, max_calls) = window;
    let outcome = evaluate_call(true, amount, spent, max_spend, calls, max_calls);
    if !crate::policy::allows_submission(&outcome) {
        return match outcome {
            PolicyOutcome::Denied { reason } => Err(PayAndCallError::PolicyDenied(reason)),
            PolicyOutcome::InsufficientBudget {
                required,
                available,
            } => Err(PayAndCallError::InsufficientBudget {
                required,
                available,
            }),
            PolicyOutcome::Allow { .. } => unreachable!(),
        };
    }
    if config.is_live() {
        retry_transient(|| -> Result<PayAndCallResult, PayAndCallError> {
            let submitted = soroban_client::pay_and_call_tx(
                config,
                &resource.contract_id,
                &resource.method,
                amount,
            )?;
            if submitted.tx_hash.starts_with("stub_") {
                return Err(PayAndCallError::TransientError(
                    "refusing stub hash on live contract path".into(),
                ));
            }
            Ok(PayAndCallResult {
                tx_hash: submitted.tx_hash,
                ledger: submitted.ledger,
                amount_spent: amount,
                resource_response: submitted.response.clone(),
                status: 200,
                body: submitted.response,
                amount,
                asset: resource.contract_id.clone(),
                pay_to: resource.contract_id.clone(),
            })
        })
    } else {
        let resp_str = format!(
            "{{\"status\": \"ok\", \"resource\": \"{}\", \"params\": {}}}",
            resource.name, params
        );
        Ok(PayAndCallResult {
            tx_hash: local_tx_hash(resource_id, params),
            ledger: 0,
            amount_spent: amount,
            resource_response: resp_str.clone(),
            status: 200,
            body: resp_str,
            amount,
            asset: resource.contract_id.clone(),
            pay_to: resource.contract_id.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Unit & Integration Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    // --- Legacy Tests Preserved ---

    #[test]
    fn test_structured_json_not_found() {
        let json = PayAndCallError::ResourceNotFound("x".into()).to_structured_json();
        assert!(json.contains("ResourceNotFound"));
        assert!(json.contains("\"code\""));
    }

    #[test]
    fn test_pay_and_call_local_success() {
        let result = pay_and_call("weather-oracle", "{}");
        assert!(result.is_ok());
        let res = result.unwrap();
        assert_eq!(res.amount_spent, 50);
        assert!(res.tx_hash.starts_with("local_"));
        assert!(!res.tx_hash.contains("stub_tx_abc123"));
    }

    #[test]
    fn test_live_path_rejects_empty_hash_via_submit() {
        let cfg = crate::soroban_client::SorobanConfig {
            account_contract_id: "CTEST".into(),
            identity: String::new(),
            ..crate::soroban_client::SorobanConfig::from_env()
        };
        let err =
            pay_and_call_with_config("weather-oracle", "{}", &cfg, (0, 0, 10_000_000, 10_000))
                .unwrap_err();
        assert!(!format!("{err}").contains("stub_tx_abc123"));
    }

    #[test]
    fn test_pay_and_call_over_cap() {
        let result = pay_and_call_with_window("weather-oracle", "{}", (0, 0, 10, 10));
        match result.unwrap_err() {
            PayAndCallError::InsufficientBudget {
                required,
                available,
            } => {
                assert_eq!(required, 50);
                assert_eq!(available, 10);
            }
            other => panic!("expected InsufficientBudget, got {other}"),
        }
    }

    #[test]
    fn test_retry_transient_then_success() {
        let mut n = 0;
        let result = retry_transient(|| {
            n += 1;
            if n < 3 {
                Err(PayAndCallError::TransientError("rpc".into()))
            } else {
                Ok(7)
            }
        });
        assert_eq!(result.unwrap(), 7);
        assert_eq!(n, 3);
    }

    #[test]
    fn test_retry_does_not_retry_policy_denied() {
        let mut n = 0;
        let result: Result<(), _> = retry_transient(|| {
            n += 1;
            Err(PayAndCallError::PolicyDenied("no".into()))
        });
        assert!(matches!(result, Err(PayAndCallError::PolicyDenied(_))));
        assert_eq!(n, 1);
    }

    #[test]
    fn test_pay_and_call_not_found() {
        let result = pay_and_call("nonexistent-resource", "{}");
        assert!(result.is_err());
        match result.unwrap_err() {
            PayAndCallError::ResourceNotFound(id) => {
                assert_eq!(id, "nonexistent-resource");
            }
            other => panic!("Expected ResourceNotFound, got: {other}"),
        }
    }

    // --- SSRF & URL Validation Tests ---

    #[test]
    fn test_ssrf_rejects_non_https() {
        let err = validate_url_and_check_ssrf("http://example.com/data", false).unwrap_err();
        assert!(matches!(err, PayAndCallError::InvalidUrl(_)));
        assert_eq!(err.code(), "InvalidUrl");
    }

    #[test]
    fn test_ssrf_rejects_localhost_and_private_ips() {
        let targets = [
            "https://127.0.0.1:8080/resource",
            "https://10.0.0.1/resource",
            "https://169.254.169.254/latest/meta-data",
            "https://[::1]/resource",
        ];
        for t in targets {
            let err = validate_url_and_check_ssrf(t, false).unwrap_err();
            assert_eq!(
                err.code(),
                "BlockedTarget",
                "Target {t} should be BlockedTarget"
            );
        }
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_ssrf_allowed_hosts_filter() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            std::env::set_var("ALLOWED_HOSTS", "api.allowed.com,data.stellar.org");
        }
        let err = validate_url_and_check_ssrf("https://unauthorized.com/res", false).unwrap_err();
        assert_eq!(err.code(), "BlockedTarget");
        unsafe {
            std::env::remove_var("ALLOWED_HOSTS");
        }
    }

    // --- Mock Server Helper for x402 Flow Tests ---

    #[allow(dead_code)]
    struct MockServer {
        port: u16,
        _handle: Option<std::thread::JoinHandle<()>>,
    }

    impl MockServer {
        fn start<F>(handler: F) -> Self
        where
            F: Fn(&mut std::net::TcpStream, usize) + Send + Sync + 'static,
        {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind tcp listener");
            let port = listener.local_addr().unwrap().port();
            let handler = std::sync::Arc::new(handler);

            let handle = std::thread::spawn(move || {
                let mut req_count = 0;
                for mut stream in listener.incoming().flatten() {
                    req_count += 1;
                    let mut buf = [0u8; 4096];
                    let _ = stream.read(&mut buf);
                    let h = handler.clone();
                    h(&mut stream, req_count);
                    let _ = stream.flush();
                }
            });

            Self {
                port,
                _handle: Some(handle),
            }
        }

        fn url(&self) -> String {
            format!("http://127.0.0.1:{}/resource", self.port)
        }
    }

    // Helper to run mock server tests with a stub payer script
    fn with_stub_payer<F>(stub_script: &str, test_fn: F)
    where
        F: FnOnce(),
    {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Write stub node script
        let tmp_dir = std::env::temp_dir();
        let script_path = tmp_dir.join(format!(
            "stub_payer_{}_{}.js",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut f = std::fs::File::create(&script_path).expect("create stub script");
        f.write_all(stub_script.as_bytes())
            .expect("write stub script");

        unsafe {
            std::env::set_var("PAYER_CMD", format!("node \"{}\"", script_path.display()));
            std::env::set_var("AGENTPAY_ALLOW_TEST_LOOPBACK", "1");
        }

        test_fn();

        unsafe {
            std::env::remove_var("PAYER_CMD");
            std::env::remove_var("AGENTPAY_ALLOW_TEST_LOOPBACK");
        }
        let _ = std::fs::remove_file(script_path);
    }

    #[test]
    fn test_free_resource_passthrough() {
        let server = MockServer::start(|stream, _| {
            let resp = "HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello, World!";
            let _ = stream.write_all(resp.as_bytes());
        });

        let res =
            pay_and_call_url_with_config(&server.url(), &SorobanConfig::default(), true).unwrap();
        assert_eq!(res.status, 200);
        assert_eq!(res.body, "Hello, World!");
        assert_eq!(res.amount, 0);
        assert!(res.tx_hash.is_empty());
    }

    #[test]
    fn test_successful_paid_call() {
        let server = MockServer::start(|stream, req_num| {
            if req_num == 1 {
                // First call: 402 with requirements
                let req_json = serde_json::json!({
                    "scheme": "exact",
                    "network": "stellar:testnet",
                    "asset": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                    "payTo": "GB7B2YWT5ELTMVDQREHAQDMIFMTKOR3TOEDOYW3ONM2ZMZGIF6CS2BA3",
                    "amount": "1000"
                });
                let body = req_json.to_string();
                let resp = format!(
                    "HTTP/1.1 402 Payment Required\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            } else {
                // Second call: 200 with settlement header
                let settlement = serde_json::json!({
                    "tx_hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    "ledger": 1234
                });
                let encoded_settlement = BASE64.encode(settlement.to_string().as_bytes());
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nPayment-Response: {}\r\nContent-Length: 14\r\n\r\nPaid Resource!",
                    encoded_settlement
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let stub = r#"
            const readline = require('readline');
            const rl = readline.createInterface({ input: process.stdin });
            let input = '';
            rl.on('line', (line) => { input += line; });
            rl.on('close', () => {
                console.log(JSON.stringify({
                    ok: true,
                    payment_payload: { transaction: 'AAAA_SIGNED' },
                    rule_id: 1,
                    header_name: 'X-PAYMENT',
                    header_value: 'ZXhhbXBsZV9wYXlsb2Fk'
                }));
            });
        "#;

        with_stub_payer(stub, || {
            let res = pay_and_call_url_with_config(&server.url(), &SorobanConfig::default(), true)
                .unwrap();
            assert_eq!(res.status, 200);
            assert_eq!(res.body, "Paid Resource!");
            assert_eq!(res.amount, 1000);
            assert_eq!(
                res.tx_hash,
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            );
        });
    }

    #[test]
    fn test_ssrf_redirect_to_private_ip_not_followed() {
        let server = MockServer::start(|stream, count| {
            if count == 1 {
                let resp = "HTTP/1.1 302 Found\r\nLocation: https://10.0.0.1/admin\r\nContent-Length: 0\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes());
            } else {
                panic!(
                    "Redirect should not be followed: unexpected request {}",
                    count
                );
            }
        });

        with_stub_payer("", || {
            let res = pay_and_call_url_with_config(&server.url(), &SorobanConfig::default(), true)
                .unwrap();
            assert_eq!(res.status, 302);
            assert_eq!(res.amount, 0);
            assert!(res.tx_hash.is_empty());
        });
    }

    #[test]
    fn test_successful_paid_call_second_call_does_not_double_pay() {
        let server = MockServer::start(|stream, req_num| {
            if req_num == 1 {
                let req_json = serde_json::json!({
                    "scheme": "exact",
                    "network": "stellar:testnet",
                    "asset": "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC",
                    "payTo": "GB7B2YWT5ELTMVDQREHAQDMIFMTKOR3TOEDOYW3ONM2ZMZGIF6CS2BA3",
                    "amount": "1000"
                });
                let body = req_json.to_string();
                let resp = format!(
                    "HTTP/1.1 402 Payment Required\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            } else if req_num == 2 {
                let settlement = serde_json::json!({
                    "tx_hash": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    "ledger": 1234
                });
                let encoded_settlement = BASE64.encode(settlement.to_string().as_bytes());
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nPayment-Response: {}\r\nContent-Length: 14\r\n\r\nPaid Resource!",
                    encoded_settlement
                );
                let _ = stream.write_all(resp.as_bytes());
            } else {
                // Subsequent call: server returns 200 directly because resource is unlocked
                let resp = "HTTP/1.1 200 OK\r\nContent-Length: 14\r\n\r\nPaid Resource!";
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let stub = r#"
            const readline = require('readline');
            const rl = readline.createInterface({ input: process.stdin });
            let input = '';
            rl.on('line', (line) => { input += line; });
            rl.on('close', () => {
                console.log(JSON.stringify({
                    ok: true,
                    payment_payload: { transaction: 'AAAA_SIGNED' },
                    rule_id: 1,
                    header_name: 'X-PAYMENT',
                    header_value: 'ZXhhbXBsZV9wYXlsb2Fk'
                }));
            });
        "#;

        with_stub_payer(stub, || {
            let res1 = pay_and_call_url_with_config(&server.url(), &SorobanConfig::default(), true)
                .unwrap();
            assert_eq!(res1.status, 200);
            assert_eq!(res1.body, "Paid Resource!");
            assert_eq!(res1.amount, 1000);
            assert_eq!(
                res1.tx_hash,
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            );

            // Second call: resource is already paid, returned without payment
            let res2 = pay_and_call_url_with_config(&server.url(), &SorobanConfig::default(), true)
                .unwrap();
            assert_eq!(res2.status, 200);
            assert_eq!(res2.body, "Paid Resource!");
            assert_eq!(res2.amount, 0);
            assert!(res2.tx_hash.is_empty());
        });
    }

    #[test]
    fn test_error_malformed_requirements() {
        let server = MockServer::start(|stream, _| {
            let resp = "HTTP/1.1 402 Payment Required\r\nContent-Type: application/json\r\nContent-Length: 22\r\n\r\n{\"malformed\": \"data\"}";
            let _ = stream.write_all(resp.as_bytes());
        });

        let err = pay_and_call_url_with_config(&server.url(), &SorobanConfig::default(), true)
            .unwrap_err();
        assert_eq!(err.code(), "PaymentRequirementsInvalid");
    }

    #[test]
    fn test_error_over_ceiling() {
        let server = MockServer::start(|stream, _| {
            let req_json = serde_json::json!({
                "network": "testnet",
                "asset": "CDLZ...",
                "payTo": "GB7B...",
                "amount": "50000000000" // exceeds 10_000_000
            });
            let body = req_json.to_string();
            let resp = format!(
                "HTTP/1.1 402 Payment Required\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        });

        let err = pay_and_call_url_with_config(&server.url(), &SorobanConfig::default(), true)
            .unwrap_err();
        assert_eq!(err.code(), "OverCeiling");
    }

    #[test]
    fn test_error_network_mismatch() {
        let server = MockServer::start(|stream, _| {
            let req_json = serde_json::json!({
                "network": "stellar:pubnet",
                "asset": "CDLZ...",
                "payTo": "GB7B...",
                "amount": "100"
            });
            let body = req_json.to_string();
            let resp = format!(
                "HTTP/1.1 402 Payment Required\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        });

        let cfg = SorobanConfig {
            network: "testnet".into(),
            ..Default::default()
        };
        let err = pay_and_call_url_with_config(&server.url(), &cfg, true).unwrap_err();
        assert_eq!(err.code(), "NetworkMismatch");
    }

    #[test]
    fn test_error_payee_not_on_policy() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let server = MockServer::start(|stream, _| {
            let req_json = serde_json::json!({
                "network": "testnet",
                "asset": "CDLZ...",
                "payTo": "G_DENIED_PAYEE",
                "amount": "100"
            });
            let body = req_json.to_string();
            let resp = format!(
                "HTTP/1.1 402 Payment Required\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        });

        unsafe {
            std::env::set_var("DENIED_PAYEES", "G_DENIED_PAYEE");
        }
        let err = pay_and_call_url_with_config(&server.url(), &SorobanConfig::default(), true)
            .unwrap_err();
        assert_eq!(err.code(), "PolicyDenied");
        unsafe {
            std::env::remove_var("DENIED_PAYEES");
        }
    }

    #[test]
    fn test_error_budget_exceeded() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let server = MockServer::start(|stream, _| {
            let req_json = serde_json::json!({
                "network": "testnet",
                "asset": "CDLZ...",
                "payTo": "GB7B...",
                "amount": "5000"
            });
            let body = req_json.to_string();
            let resp = format!(
                "HTTP/1.1 402 Payment Required\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        });

        // Set state with remaining_stroops = 100
        let tmp_state =
            std::env::temp_dir().join(format!("test_budget_state_{}.json", std::process::id()));
        std::fs::write(&tmp_state, r#"{"remaining_stroops": 100, "remaining_xlm": 0.00001, "period_ledgers": 100, "rule_count": 1}"#).unwrap();
        unsafe {
            std::env::set_var("AGENTPAY_STATE", tmp_state.to_str().unwrap());
        }

        let err = pay_and_call_url_with_config(&server.url(), &SorobanConfig::default(), true)
            .unwrap_err();
        assert_eq!(err.code(), "BudgetExceeded");

        unsafe {
            std::env::remove_var("AGENTPAY_STATE");
        }
        let _ = std::fs::remove_file(tmp_state);
    }

    #[test]
    fn test_error_payer_failure() {
        let server = MockServer::start(|stream, _| {
            let req_json = serde_json::json!({
                "network": "testnet",
                "asset": "CDLZ...",
                "payTo": "GB7B...",
                "amount": "100"
            });
            let body = req_json.to_string();
            let resp = format!(
                "HTTP/1.1 402 Payment Required\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        });

        let stub = r#"
            console.log(JSON.stringify({ ok: false, code: "SIMULATION_FAILED", message: "sim error" }));
            process.exit(1);
        "#;

        with_stub_payer(stub, || {
            let err = pay_and_call_url_with_config(&server.url(), &SorobanConfig::default(), true)
                .unwrap_err();
            assert_eq!(err.code(), "PayerFailed");
        });
    }

    #[test]
    fn test_error_payer_timeout() {
        let server = MockServer::start(|stream, _| {
            let req_json = serde_json::json!({
                "network": "testnet",
                "asset": "CDLZ...",
                "payTo": "GB7B...",
                "amount": "100"
            });
            let body = req_json.to_string();
            let resp = format!(
                "HTTP/1.1 402 Payment Required\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        });

        let stub = r#"
            // sleep for 2 seconds
            setTimeout(() => {}, 2000);
        "#;

        with_stub_payer(stub, || {
            unsafe {
                std::env::set_var("PAYER_TIMEOUT_MS", "50");
            }
            let err = pay_and_call_url_with_config(&server.url(), &SorobanConfig::default(), true)
                .unwrap_err();
            assert_eq!(err.code(), "PayerTimeout");
            unsafe {
                std::env::remove_var("PAYER_TIMEOUT_MS");
            }
        });
    }

    #[test]
    fn test_error_second_402_after_payment_no_loop() {
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_clone = call_count.clone();

        let server = MockServer::start(move |stream, _| {
            count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let req_json = serde_json::json!({
                "network": "testnet",
                "asset": "CDLZ...",
                "payTo": "GB7B...",
                "amount": "100"
            });
            let body = req_json.to_string();
            let resp = format!(
                "HTTP/1.1 402 Payment Required\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        });

        let stub = r#"
            console.log(JSON.stringify({
                ok: true,
                payment_payload: { transaction: 'AAAA' },
                header_name: 'X-PAYMENT',
                header_value: 'cGF5bG9hZA=='
            }));
        "#;

        with_stub_payer(stub, || {
            let err = pay_and_call_url_with_config(&server.url(), &SorobanConfig::default(), true)
                .unwrap_err();
            assert_eq!(err.code(), "FacilitatorRejected");
            assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
        });
    }

    #[test]
    fn test_error_missing_settlement_header() {
        let server = MockServer::start(|stream, req_num| {
            if req_num == 1 {
                let req_json = serde_json::json!({
                    "network": "testnet",
                    "asset": "CDLZ...",
                    "payTo": "GB7B...",
                    "amount": "100"
                });
                let body = req_json.to_string();
                let resp = format!(
                    "HTTP/1.1 402 Payment Required\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            } else {
                // Returns 200 without Payment-Response header
                let resp = "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nDone";
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let stub = r#"
            console.log(JSON.stringify({
                ok: true,
                payment_payload: { transaction: 'AAAA' },
                header_name: 'X-PAYMENT',
                header_value: 'cGF5bG9hZA=='
            }));
        "#;

        with_stub_payer(stub, || {
            let err = pay_and_call_url_with_config(&server.url(), &SorobanConfig::default(), true)
                .unwrap_err();
            assert_eq!(err.code(), "SettlementMissing");
        });
    }

    #[test]
    fn test_real_testnet_gated_by_env() {
        if std::env::var("RUN_TESTNET_TESTS").unwrap_or_default() != "true" {
            // Skipped by default as specified in acceptance criteria
            return;
        }
        // Testnet test logic would run here if flag set
        let cfg = SorobanConfig::from_env();
        assert!(!cfg.network.is_empty());
    }
}

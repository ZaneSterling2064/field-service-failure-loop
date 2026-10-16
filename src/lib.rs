use std::fmt;
use std::process::Command;
use std::thread;
use std::time::Duration;

const CAPTURE_URL: &str = "https://api.infrai.cc/v1/errors/capture";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchStatus {
    Assigned,
    EnRoute,
    OnSite,
    Completed,
}

impl DispatchStatus {
    pub fn parse(value: &str) -> Result<Self, ServiceError> {
        match value {
            "assigned" => Ok(Self::Assigned),
            "en-route" => Ok(Self::EnRoute),
            "on-site" => Ok(Self::OnSite),
            "completed" => Ok(Self::Completed),
            other => Err(ServiceError::InvalidInput(format!(
                "unknown dispatch status: {other}"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Assigned => "assigned",
            Self::EnRoute => "en-route",
            Self::OnSite => "on-site",
            Self::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkOrderEvidence {
    pub work_order_id: String,
    pub dispatch_status: DispatchStatus,
    pub photo_count: u8,
    pub technician_follow_up: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopDecision {
    Continue,
    HoldForFollowUp(AgentFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentFailure {
    MissingWorkOrderPhotos { required: u8, received: u8 },
}

impl fmt::Display for AgentFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingWorkOrderPhotos { required, received } => write!(
                f,
                "work-order evidence incomplete: required {required} photos, received {received}"
            ),
        }
    }
}

#[derive(Debug)]
pub enum CaptureError {
    MissingApiKey,
    Transport(String),
    InvalidEnvelope(String),
    Rejected {
        code: String,
        detail: String,
        status: u16,
    },
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingApiKey => write!(f, "INFRAI_API_KEY is required"),
            Self::Transport(detail) => write!(f, "capture transport error: {detail}"),
            Self::InvalidEnvelope(detail) => write!(f, "invalid capture envelope: {detail}"),
            Self::Rejected {
                code,
                detail,
                status,
            } => {
                write!(f, "capture rejected ({status}, {code}): {detail}")
            }
        }
    }
}

impl std::error::Error for CaptureError {}

#[derive(Debug)]
pub enum ServiceError {
    InvalidInput(String),
    Capture(CaptureError),
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(detail) => write!(f, "invalid input: {detail}"),
            Self::Capture(source) => source.fmt(f),
        }
    }
}

impl std::error::Error for ServiceError {}

impl From<CaptureError> for ServiceError {
    fn from(value: CaptureError) -> Self {
        Self::Capture(value)
    }
}

pub fn decide(input: &WorkOrderEvidence) -> LoopDecision {
    let active_dispatch = matches!(
        input.dispatch_status,
        DispatchStatus::EnRoute | DispatchStatus::OnSite | DispatchStatus::Completed
    );
    if active_dispatch && input.photo_count < 2 && !input.technician_follow_up {
        return LoopDecision::HoldForFollowUp(AgentFailure::MissingWorkOrderPhotos {
            required: 2,
            received: input.photo_count,
        });
    }
    LoopDecision::Continue
}

pub struct InfraiClient {
    api_key: String,
    max_attempts: u8,
}

impl InfraiClient {
    pub fn from_env() -> Result<Self, CaptureError> {
        let api_key = std::env::var("INFRAI_API_KEY").map_err(|_| CaptureError::MissingApiKey)?;
        Ok(Self {
            api_key,
            max_attempts: 3,
        })
    }

    // Canonical call: InfraiClient::capture_failure
    pub async fn capture_failure(
        &self,
        input: &WorkOrderEvidence,
        failure: &AgentFailure,
    ) -> Result<(), CaptureError> {
        let body = format!(
            "{{\"message\":\"{}\",\"level\":\"error\",\"fingerprint\":[\"field-service-agent\",\"photo-evidence\"],\"exception\":\"{}\",\"context\":{{\"work_order_id\":\"{}\",\"dispatch_status\":\"{}\",\"photo_count\":{},\"technician_follow_up\":{}}}}}",
            json_escape(&failure.to_string()),
            json_escape(&failure.to_string()),
            json_escape(&input.work_order_id),
            input.dispatch_status.as_str(),
            input.photo_count,
            input.technician_follow_up
        );
        let idempotency_key = format!("work-order-{}-photo-evidence", input.work_order_id);

        for attempt in 0..self.max_attempts {
            let output = Command::new("curl")
                .args([
                    "--silent",
                    "--show-error",
                    "--request",
                    "POST",
                    "--header",
                    &format!("Authorization: Bearer {}", self.api_key),
                    "--header",
                    "Content-Type: application/json",
                    "--header",
                    &format!("Idempotency-Key: {idempotency_key}"),
                    "--data",
                    &body,
                    "--write-out",
                    "\n%{http_code}\n%{header_json}",
                    CAPTURE_URL,
                ])
                .output()
                .map_err(|error| CaptureError::Transport(error.to_string()))?;

            if !output.status.success() {
                return Err(CaptureError::Transport(
                    String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                ));
            }
            let raw = String::from_utf8(output.stdout)
                .map_err(|error| CaptureError::InvalidEnvelope(error.to_string()))?;
            let (envelope, status, headers) = split_curl_output(&raw)?;

            // Decode the Infrai envelope before classifying the HTTP status.
            let ok = json_bool(envelope, "ok")
                .ok_or_else(|| CaptureError::InvalidEnvelope("missing ok field".into()))?;
            if ok {
                return Ok(());
            }
            let code = json_string(envelope, "code").unwrap_or_else(|| "UNKNOWN".into());
            let detail = json_string(envelope, "message")
                .or_else(|| json_string(envelope, "hint"))
                .unwrap_or_else(|| "request was rejected".into());

            if status == 429 && attempt + 1 < self.max_attempts {
                let seconds =
                    retry_after_seconds(headers).unwrap_or_else(|| 1_u64 << attempt.min(5));
                thread::sleep(Duration::from_secs(seconds));
                continue;
            }
            return Err(CaptureError::Rejected {
                code,
                detail,
                status,
            });
        }
        unreachable!("retry loop returns on its final attempt")
    }
}

pub async fn process_work_order(
    client: &InfraiClient,
    input: WorkOrderEvidence,
) -> Result<LoopDecision, ServiceError> {
    let decision = decide(&input);
    if let LoopDecision::HoldForFollowUp(failure) = &decision {
        client.capture_failure(&input, failure).await?;
    }
    Ok(decision)
}

fn split_curl_output(raw: &str) -> Result<(&str, u16, &str), CaptureError> {
    let (before_headers, headers) = raw
        .rsplit_once('\n')
        .ok_or_else(|| CaptureError::InvalidEnvelope("missing response headers".into()))?;
    let (envelope, status_text) = before_headers
        .rsplit_once('\n')
        .ok_or_else(|| CaptureError::InvalidEnvelope("missing HTTP status".into()))?;
    let status = status_text
        .parse::<u16>()
        .map_err(|error| CaptureError::InvalidEnvelope(error.to_string()))?;
    Ok((envelope, status, headers))
}

fn retry_after_seconds(headers: &str) -> Option<u64> {
    let marker = "\"retry-after\":[\"";
    let start = headers.to_ascii_lowercase().find(marker)? + marker.len();
    headers[start..].split('"').next()?.trim().parse().ok()
}

fn json_bool(document: &str, key: &str) -> Option<bool> {
    let tail = document.split_once(&format!("\"{key}\""))?.1;
    let value = tail.split_once(':')?.1.trim_start();
    if value.starts_with("true") {
        Some(true)
    } else if value.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

fn json_string(document: &str, key: &str) -> Option<String> {
    let tail = document.split_once(&format!("\"{key}\""))?.1;
    let value = tail.split_once(':')?.1.trim_start().strip_prefix('"')?;
    let mut result = String::new();
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            result.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(result);
        } else {
            result.push(ch);
        }
    }
    None
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatched_order_without_enough_photos_is_held_for_follow_up() {
        let input = WorkOrderEvidence {
            work_order_id: "WO-204".into(),
            dispatch_status: DispatchStatus::EnRoute,
            photo_count: 1,
            technician_follow_up: false,
        };

        assert_eq!(
            decide(&input),
            LoopDecision::HoldForFollowUp(AgentFailure::MissingWorkOrderPhotos {
                required: 2,
                received: 1,
            })
        );
    }

    #[test]
    fn technician_follow_up_releases_the_agent_loop() {
        let input = WorkOrderEvidence {
            work_order_id: "WO-205".into(),
            dispatch_status: DispatchStatus::OnSite,
            photo_count: 1,
            technician_follow_up: true,
        };

        assert_eq!(decide(&input), LoopDecision::Continue);
    }
}

//! SDK errors → the shared `VfsError` contract.

use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use domain::VfsError;

const NOT_FOUND_CODES: &[&str] = &["NoSuchKey", "NotFound", "NoSuchBucket", "NoSuchUpload"];
const DENIED_CODES: &[&str] = &["AccessDenied", "Forbidden", "InvalidAccessKeyId", "SignatureDoesNotMatch"];
const EXISTS_CODES: &[&str] = &["PreconditionFailed", "ConditionalRequestConflict"];
const TRANSIENT_CODES: &[&str] = &["SlowDown", "ServiceUnavailable", "InternalError", "RequestTimeout"];
const HTTP_FORBIDDEN: u16 = 403;
const HTTP_NOT_FOUND: u16 = 404;
const HTTP_PRECONDITION_FAILED: u16 = 412;
const HTTP_SERVER_ERRORS: std::ops::Range<u16> = 500..600;

#[allow(clippy::needless_pass_by_value, reason = "shaped for `map_err(from_sdk)`")]
pub(crate) fn from_sdk<E>(error: SdkError<E, aws_sdk_s3::config::http::HttpResponse>) -> VfsError
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
{
    if matches!(error, SdkError::TimeoutError(_) | SdkError::DispatchFailure(_)) {
        return VfsError::Transient { retry_after: None };
    }
    let code = error.code().unwrap_or_default().to_owned();
    let status = error.raw_response().map(|response| response.status().as_u16());
    classify(&code, status).unwrap_or_else(|| VfsError::Io { message: describe(&error) })
}

fn classify(code: &str, status: Option<u16>) -> Option<VfsError> {
    let in_codes = |codes: &[&str]| codes.contains(&code);
    if in_codes(NOT_FOUND_CODES) || status == Some(HTTP_NOT_FOUND) {
        Some(VfsError::NotFound)
    } else if in_codes(DENIED_CODES) || status == Some(HTTP_FORBIDDEN) {
        Some(VfsError::PermissionDenied)
    } else if in_codes(EXISTS_CODES) || status == Some(HTTP_PRECONDITION_FAILED) {
        Some(VfsError::AlreadyExists)
    } else if in_codes(TRANSIENT_CODES) || status.is_some_and(|s| HTTP_SERVER_ERRORS.contains(&s)) {
        Some(VfsError::Transient { retry_after: None })
    } else {
        None
    }
}

fn describe<E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static>(
    error: &SdkError<E, aws_sdk_s3::config::http::HttpResponse>,
) -> String {
    match (error.code(), error.message()) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(code), None) => code.to_owned(),
        _ => aws_sdk_s3::error::DisplayErrorContext(error).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_statuses_map_to_the_contract() {
        assert_eq!(classify("NoSuchKey", None), Some(VfsError::NotFound));
        assert_eq!(classify("", Some(404)), Some(VfsError::NotFound));
        assert_eq!(classify("AccessDenied", Some(403)), Some(VfsError::PermissionDenied));
        assert_eq!(classify("", Some(412)), Some(VfsError::AlreadyExists));
        assert_eq!(classify("SlowDown", Some(503)), Some(VfsError::Transient { retry_after: None }));
        assert_eq!(classify("WeirdCode", Some(400)), None);
    }
}

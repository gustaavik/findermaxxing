//! A Keygen double: licenses by key, machines with an activation limit,
//! and machine files signed with a test Ed25519 key exactly as Keygen signs
//! them (`base64+ed25519` over `machine/<enc>`).

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signer, SigningKey};
use hyper::StatusCode;
use serde_json::{Value, json};

use crate::State;
use crate::http::{Reply, Request};

/// A fixed test key; never used for anything real.
const SIGNING_SEED: [u8; 32] = [7; 32];
const FILE_TTL_SECONDS: i64 = 30 * 86_400;

#[derive(Debug, Clone)]
pub struct MockLicense {
    pub id: String,
    pub key: String,
    pub name: String,
    /// `ACTIVE`, `SUSPENDED`, `EXPIRED`.
    pub status: String,
    pub max_machines: usize,
    pub machines: Vec<String>,
}

#[derive(Debug, Default)]
pub struct KeygenState {
    pub licenses: Vec<MockLicense>,
    pub requests: Vec<String>,
}

/// The verify key the app must be built with to trust this mock.
#[must_use]
pub fn public_key_hex() -> String {
    SigningKey::from_bytes(&SIGNING_SEED).verifying_key().as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn errors(status: StatusCode, code: &str) -> Reply {
    Reply::json(status, &json!({"errors": [{"code": code, "detail": code}]}))
}

fn rfc3339(seconds: i64) -> String {
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z", rest / 3600, rest / 60 % 60, rest % 60)
}

fn now_seconds() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

fn licensed<'a>(state: &'a mut State, request: &Request) -> Option<&'a mut MockLicense> {
    let key = request.header("authorization")?.strip_prefix("License ")?.to_owned();
    state.keygen.licenses.iter_mut().find(|l| l.key == key)
}

fn license_json(license: &MockLicense) -> Value {
    json!({"type": "licenses", "id": license.id, "attributes": {"name": license.name, "status": license.status, "expiry": null}})
}

fn machine_file(license: &MockLicense, fingerprint: &str) -> String {
    let issued = now_seconds();
    let payload = json!({
        "data": {"type": "machines", "id": format!("m-{fingerprint}"), "attributes": {"fingerprint": fingerprint}},
        "included": [license_json(license)],
        "meta": {"issued": rfc3339(issued), "expiry": rfc3339(issued + FILE_TTL_SECONDS), "ttl": FILE_TTL_SECONDS},
    });
    let enc = STANDARD.encode(payload.to_string());
    let sig =
        STANDARD.encode(SigningKey::from_bytes(&SIGNING_SEED).sign(format!("machine/{enc}").as_bytes()).to_bytes());
    let body = STANDARD.encode(json!({"enc": enc, "sig": sig, "alg": "base64+ed25519"}).to_string());
    let wrapped: Vec<String> = body.as_bytes().chunks(80).map(|c| String::from_utf8_lossy(c).into_owned()).collect();
    format!("-----BEGIN MACHINE FILE-----\n{}\n-----END MACHINE FILE-----\n", wrapped.join("\n"))
}

pub(crate) fn handle(state: &mut State, rest: &str, request: &Request) -> Reply {
    state.keygen.requests.push(format!("{} {rest}", request.method));
    let path = rest.split_once('/').map_or("", |(_, path)| path);
    match (request.method.as_str(), path) {
        ("POST", "licenses/actions/validate-key") => validate(state, request),
        ("POST", "machines") => activate(state, request),
        ("POST", machine) if machine.starts_with("machines/") && machine.ends_with("/actions/check-out") => {
            let fingerprint = machine.trim_start_matches("machines/").trim_end_matches("/actions/check-out");
            check_out(state, request, fingerprint)
        }
        ("DELETE", machine) if machine.starts_with("machines/") => {
            deactivate(state, request, machine.trim_start_matches("machines/"))
        }
        _ => errors(StatusCode::NOT_FOUND, "NOT_FOUND"),
    }
}

fn validate(state: &State, request: &Request) -> Reply {
    let body = request.json();
    let key = body["meta"]["key"].as_str().unwrap_or_default();
    let fingerprint = body["meta"]["scope"]["fingerprint"].as_str().unwrap_or_default();
    let Some(license) = state.keygen.licenses.iter().find(|l| l.key == key) else {
        return Reply::ok(&json!({"data": null, "meta": {"valid": false, "code": "NOT_FOUND"}}));
    };
    let code = if license.status != "ACTIVE" {
        license.status.as_str()
    } else if license.machines.iter().any(|m| m == fingerprint) {
        "VALID"
    } else if license.machines.is_empty() {
        "NO_MACHINES"
    } else {
        "FINGERPRINT_SCOPE_MISMATCH"
    };
    Reply::ok(&json!({"data": license_json(license), "meta": {"valid": code == "VALID", "code": code}}))
}

fn activate(state: &mut State, request: &Request) -> Reply {
    let fingerprint = request.json()["data"]["attributes"]["fingerprint"].as_str().unwrap_or_default().to_owned();
    let Some(license) = licensed(state, request) else {
        return errors(StatusCode::UNAUTHORIZED, "LICENSE_INVALID");
    };
    if license.machines.contains(&fingerprint) {
        return errors(StatusCode::UNPROCESSABLE_ENTITY, "FINGERPRINT_TAKEN");
    }
    if license.machines.len() >= license.max_machines {
        return errors(StatusCode::UNPROCESSABLE_ENTITY, "MACHINE_LIMIT_EXCEEDED");
    }
    license.machines.push(fingerprint.clone());
    Reply::json(StatusCode::CREATED, &json!({"data": {"type": "machines", "attributes": {"fingerprint": fingerprint}}}))
}

fn check_out(state: &mut State, request: &Request, fingerprint: &str) -> Reply {
    let Some(license) = licensed(state, request) else {
        return errors(StatusCode::UNAUTHORIZED, "LICENSE_INVALID");
    };
    if license.status != "ACTIVE" {
        return errors(StatusCode::FORBIDDEN, "LICENSE_SUSPENDED");
    }
    if !license.machines.iter().any(|m| m == fingerprint) {
        return errors(StatusCode::NOT_FOUND, "NOT_FOUND");
    }
    let certificate = machine_file(license, fingerprint);
    Reply::ok(&json!({"data": {"type": "machine-files", "attributes": {"certificate": certificate}}}))
}

fn deactivate(state: &mut State, request: &Request, fingerprint: &str) -> Reply {
    let Some(license) = licensed(state, request) else {
        return errors(StatusCode::UNAUTHORIZED, "LICENSE_INVALID");
    };
    let before = license.machines.len();
    license.machines.retain(|m| m != fingerprint);
    if license.machines.len() == before {
        errors(StatusCode::NOT_FOUND, "NOT_FOUND")
    } else {
        Reply::empty(StatusCode::NO_CONTENT)
    }
}

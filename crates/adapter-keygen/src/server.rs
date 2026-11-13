//! The Keygen API calls a client makes with its license key.

use std::time::Duration;

use application::ports::{LicenseError, LicenseServer, MachineFile, Secret};
use futures::FutureExt;
use futures::future::BoxFuture;
use reqwest::StatusCode;
use serde_json::{Value, json};

const JSON_API: &str = "application/vnd.api+json";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The machine file includes the license, so expiry and status work offline.
const CHECKOUT_QUERY: &str = "include=license&algorithm=base64%2Bed25519";
/// Validation codes meaning "valid, but not activated on this machine yet".
const NOT_ACTIVATED_HERE: &[&str] = &["NO_MACHINE", "NO_MACHINES", "FINGERPRINT_SCOPE_MISMATCH"];
const ENDED: &[&str] = &["SUSPENDED", "EXPIRED", "BANNED", "OVERDUE"];
const MACHINE_LIMIT: &[&str] = &["TOO_MANY_MACHINES", "MACHINE_LIMIT_EXCEEDED"];
/// The machine is already activated (a retry after a lost answer).
const FINGERPRINT_TAKEN: &str = "FINGERPRINT_TAKEN";

/// Which Keygen account licenses the app (compiled in, ADR 0030).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeygenAccount {
    /// `https://api.keygen.sh` (or a self-hosted Keygen CE/EE).
    pub base_url: String,
    /// Account id or slug.
    pub account: String,
}

#[derive(Debug, Clone)]
pub struct KeygenServer {
    account: KeygenAccount,
    client: reqwest::Client,
}

impl KeygenServer {
    /// # Errors
    /// The HTTP client couldn't be built.
    pub fn new(account: KeygenAccount) -> Result<Self, reqwest::Error> {
        Ok(Self { account, client: reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build()? })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1/accounts/{}/{path}", self.account.base_url.trim_end_matches('/'), self.account.account)
    }
}

fn network(error: reqwest::Error) -> LicenseError {
    LicenseError::Network(error.without_url().to_string())
}

/// The first error code of a JSON:API error document.
fn error_code(body: &Value) -> String {
    body["errors"][0]["code"].as_str().unwrap_or_default().to_owned()
}

fn error_for(status: StatusCode, body: &Value) -> LicenseError {
    let code = error_code(body);
    if MACHINE_LIMIT.contains(&code.as_str()) {
        return LicenseError::MachineLimit;
    }
    match status.as_u16() {
        401 | 404 => LicenseError::UnknownKey,
        403 => LicenseError::Ended,
        _ => {
            let detail = body["errors"][0]["detail"].as_str().unwrap_or_default();
            LicenseError::Service(if detail.is_empty() { format!("HTTP {status}") } else { detail.to_owned() })
        }
    }
}

async fn send(request: reqwest::RequestBuilder) -> Result<(StatusCode, Value), LicenseError> {
    let response = request.header("Accept", JSON_API).send().await.map_err(network)?;
    let status = response.status();
    let body = response.json().await.unwrap_or(Value::Null);
    Ok((status, body))
}

impl KeygenServer {
    /// The license id if the key is valid for this machine (`Ok(None)`:
    /// valid but not activated here yet).
    async fn validate(&self, key: &Secret, fingerprint: &str) -> Result<(String, bool), LicenseError> {
        let body = json!({"meta": {"key": key.expose(), "scope": {"fingerprint": fingerprint}}});
        let request =
            self.client.post(self.url("licenses/actions/validate-key")).header("Content-Type", JSON_API).json(&body);
        let (status, answer) = send(request).await?;
        if !status.is_success() {
            return Err(error_for(status, &answer));
        }
        let code = answer["meta"]["code"].as_str().unwrap_or_default();
        let Some(id) = answer["data"]["id"].as_str() else { return Err(LicenseError::UnknownKey) };
        match code {
            "VALID" => Ok((id.to_owned(), true)),
            code if NOT_ACTIVATED_HERE.contains(&code) => Ok((id.to_owned(), false)),
            code if ENDED.contains(&code) => Err(LicenseError::Ended),
            code if MACHINE_LIMIT.contains(&code) => Err(LicenseError::MachineLimit),
            _ => Err(LicenseError::UnknownKey),
        }
    }

    async fn activate_machine(
        &self,
        key: &Secret,
        license: &str,
        fingerprint: &str,
        name: &str,
    ) -> Result<(), LicenseError> {
        let body = json!({"data": {
            "type": "machines",
            "attributes": {"fingerprint": fingerprint, "platform": std::env::consts::OS, "name": name},
            "relationships": {"license": {"data": {"type": "licenses", "id": license}}},
        }});
        let request = self
            .client
            .post(self.url("machines"))
            .header("Authorization", format!("License {}", key.expose()))
            .header("Content-Type", JSON_API)
            .json(&body);
        match send(request).await? {
            (status, _) if status.is_success() => Ok(()),
            (_, answer) if error_code(&answer) == FINGERPRINT_TAKEN => Ok(()),
            (status, answer) => Err(error_for(status, &answer)),
        }
    }

    async fn check_out(&self, key: &Secret, fingerprint: &str) -> Result<MachineFile, LicenseError> {
        let url = format!("{}?{CHECKOUT_QUERY}", self.url(&format!("machines/{fingerprint}/actions/check-out")));
        let request = self.client.post(url).header("Authorization", format!("License {}", key.expose()));
        let (status, answer) = send(request).await?;
        if !status.is_success() {
            return Err(error_for(status, &answer));
        }
        answer["data"]["attributes"]["certificate"]
            .as_str()
            .map(|c| MachineFile(c.to_owned()))
            .ok_or_else(|| LicenseError::Service("no machine file in the answer".into()))
    }
}

impl LicenseServer for KeygenServer {
    fn activate(
        &self,
        key: &Secret,
        fingerprint: &str,
        machine_name: &str,
    ) -> BoxFuture<'static, Result<MachineFile, LicenseError>> {
        let (server, key, fingerprint, name) =
            (self.clone(), key.clone(), fingerprint.to_owned(), machine_name.to_owned());
        async move {
            let (license, activated) = server.validate(&key, &fingerprint).await?;
            if !activated {
                server.activate_machine(&key, &license, &fingerprint, &name).await?;
            }
            server.check_out(&key, &fingerprint).await
        }
        .boxed()
    }

    fn renew(&self, key: &Secret, fingerprint: &str) -> BoxFuture<'static, Result<MachineFile, LicenseError>> {
        let (server, key, fingerprint) = (self.clone(), key.clone(), fingerprint.to_owned());
        async move { server.check_out(&key, &fingerprint).await }.boxed()
    }

    fn deactivate(&self, key: &Secret, fingerprint: &str) -> BoxFuture<'static, Result<(), LicenseError>> {
        let (server, key, fingerprint) = (self.clone(), key.clone(), fingerprint.to_owned());
        async move {
            let request = server
                .client
                .delete(server.url(&format!("machines/{fingerprint}")))
                .header("Authorization", format!("License {}", key.expose()));
            match send(request).await? {
                (status, _) if status.is_success() => Ok(()),
                (status, answer) => Err(error_for(status, &answer)),
            }
        }
        .boxed()
    }
}

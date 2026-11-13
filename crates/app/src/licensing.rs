//! Licensing (ADR 0030): Keygen, configured at build time.
//!
//! - `FM_KEYGEN_ACCOUNT`: the account id or slug;
//! - `FM_KEYGEN_PUBLIC_KEY`: its Ed25519 verify key (hex), which machine files must be signed with;
//! - `FM_KEYGEN_URL`: the API, `https://api.keygen.sh` unless self-hosted;
//! - `FM_STORE_URL`: where "Buy Pro" leads.
//!
//! Debug builds also read them from the environment. Without an account,
//! licensing is off (debug builds unlock Pro; release builds stay free).

use std::sync::Arc;

use adapter_keygen::{HostIdentity, KeygenAccount, KeygenMachineFiles, KeygenServer};
use adapter_sqlite_store::SqliteStore;
use application::ports::CredentialStore;
use application::use_cases::LicenseServices;
use presentation::{LicenseUseCases, Licensing};

use crate::drives::client_setting;

const DEFAULT_KEYGEN_URL: &str = "https://api.keygen.sh";

pub(crate) fn build(store: &Arc<SqliteStore>, credentials: &Arc<dyn CredentialStore>) -> Licensing {
    let store_url = client_setting(option_env!("FM_STORE_URL"), "FM_STORE_URL").unwrap_or_default();
    let use_cases = configured(store, credentials);
    if use_cases.is_none() {
        eprintln!("FinderMaxxing: licensing isn't configured in this build (FM_KEYGEN_ACCOUNT, FM_KEYGEN_PUBLIC_KEY).");
    }
    Licensing { use_cases, store_url }
}

fn configured(store: &Arc<SqliteStore>, credentials: &Arc<dyn CredentialStore>) -> Option<LicenseUseCases> {
    let account = client_setting(option_env!("FM_KEYGEN_ACCOUNT"), "FM_KEYGEN_ACCOUNT")?;
    let public_key = client_setting(option_env!("FM_KEYGEN_PUBLIC_KEY"), "FM_KEYGEN_PUBLIC_KEY")?;
    let base_url =
        client_setting(option_env!("FM_KEYGEN_URL"), "FM_KEYGEN_URL").unwrap_or_else(|| DEFAULT_KEYGEN_URL.to_owned());
    let reader = match KeygenMachineFiles::new(&public_key) {
        Ok(reader) => reader,
        Err(error) => {
            eprintln!("FinderMaxxing: FM_KEYGEN_PUBLIC_KEY is invalid ({error}); licensing is off.");
            return None;
        }
    };
    let server = KeygenServer::new(KeygenAccount { base_url, account }).ok()?;
    let services = LicenseServices {
        server: Arc::new(server),
        reader: Arc::new(reader),
        store: store.clone(),
        credentials: credentials.clone(),
        identity: Arc::new(HostIdentity::detect()?),
    };
    Some(LicenseUseCases::new(&services))
}

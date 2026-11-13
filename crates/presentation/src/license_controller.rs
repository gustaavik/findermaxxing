//! `LicenseController`: the edition shown in the window, the License
//! dialog (activate, deactivate), and the gate in front of Pro features:
//! `require(feature)` answers, and asks for an upgrade when it says no.

use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use application::ports::LicenseError;
use cxx_qt::Threading;
use cxx_qt_lib::QString;
use domain::{LicenseStatus, ProFeature, Timestamp};

use crate::services::AppServices;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("presentation/cpp/services_holder.h");
        type ServicesHolder = crate::services_bridge::ServicesHolder;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(*mut ServicesHolder, services)]
        #[qproperty(bool, pro)]
        /// "Free", "Pro", or "Pro (developer build)".
        #[qproperty(QString, edition)]
        /// One line about the license (who, until when, what to do).
        #[qproperty(QString, status_text)]
        #[qproperty(bool, activated)]
        #[qproperty(bool, busy)]
        #[qproperty(QString, store_url)]
        #[qproperty(QString, error_message)]
        type LicenseController = super::LicenseControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        /// Reads the license and renews its file if it's due (needs network then).
        #[qinvokable]
        fn refresh(self: Pin<&mut LicenseController>);
        #[qinvokable]
        fn activate(self: Pin<&mut LicenseController>, key: &QString);
        #[qinvokable]
        fn deactivate(self: Pin<&mut LicenseController>);
        /// Whether `feature` (a `ProFeature` key) is unlocked; if not, asks
        /// the window to offer an upgrade.
        #[qinvokable]
        fn require(self: Pin<&mut LicenseController>, feature: &QString) -> bool;

        #[qsignal]
        fn upgrade_requested(self: Pin<&mut LicenseController>, feature: QString);
    }

    impl cxx_qt::Threading for LicenseController {}
}

use qobject::ServicesHolder;

const DEVELOPER_EDITION: &str = "Pro (developer build)";
const NOT_CONFIGURED_NOTE: &str = "This build has no license server; Pro is unavailable.";

// The bools are Q_PROPERTY storage, which cxx-qt requires as plain fields.
#[allow(clippy::struct_excessive_bools, reason = "mirrors Qt properties one-to-one")]
pub struct LicenseControllerRust {
    services: *mut ServicesHolder,
    pro: bool,
    edition: QString,
    status_text: QString,
    activated: bool,
    busy: bool,
    store_url: QString,
    error_message: QString,
}

impl Default for LicenseControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            pro: false,
            edition: QString::from("Free"),
            status_text: QString::default(),
            activated: false,
            busy: false,
            store_url: QString::default(),
            error_message: QString::default(),
        }
    }
}

fn now() -> Timestamp {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    Timestamp::from_unix_millis(i64::try_from(millis).unwrap_or(i64::MAX))
}

/// (edition, status line, activated here).
fn describe(status: &LicenseStatus) -> (&'static str, String, bool) {
    match status {
        LicenseStatus::Free => {
            ("Free", "Local browsing, file operations, previews and search are free.".to_owned(), false)
        }
        LicenseStatus::Pro { licensee, license_expiry: None } => ("Pro", format!("Licensed to {licensee}."), true),
        LicenseStatus::Pro { licensee, license_expiry: Some(end) } => {
            let end = end.civil(0);
            ("Pro", format!("Licensed to {licensee} until {:04}-{:02}-{:02}.", end.year, end.month, end.day), true)
        }
        LicenseStatus::Ended { licensee } => ("Free", format!("The license of {licensee} has ended."), true),
        LicenseStatus::NeedsConfirmation { licensee } => (
            "Free",
            format!(
                "The license of {licensee} hasn't been confirmed for a month. Connect to the internet to continue."
            ),
            true,
        ),
        LicenseStatus::OtherMachine => ("Free", "The saved license belongs to another computer.".to_owned(), false),
        LicenseStatus::ClockSetBack => {
            ("Free", "The computer's clock is behind; correct it to use Pro.".to_owned(), true)
        }
    }
}

fn activation_message(error: &LicenseError) -> String {
    match error {
        LicenseError::Network(_) => {
            "The license server couldn't be reached. Check the connection and try again.".to_owned()
        }
        other => format!("{}.", capitalised(&other.to_string())),
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

impl qobject::LicenseController {
    pub fn refresh(mut self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return };
        self.as_mut().set_store_url(QString::from(&services.licensing.store_url));
        let Some(licensing) = services.licensing.use_cases.clone() else {
            let developer = cfg!(debug_assertions);
            self.as_mut().set_pro(developer);
            self.as_mut().set_edition(QString::from(if developer { DEVELOPER_EDITION } else { "Free" }));
            self.set_status_text(QString::from(NOT_CONFIGURED_NOTE));
            return;
        };
        let thread = self.qt_thread();
        services.runtime.spawn(async move {
            // Offline answer first, so the window never waits on the network.
            if let Ok(status) = licensing.current.execute(now()).await {
                let _ = thread.queue(move |controller| controller.show(&status));
            }
            if let Ok(status) = licensing.renew.execute(now()).await {
                let _ = thread.queue(move |controller| controller.show(&status));
            }
        });
    }

    pub fn activate(mut self: Pin<&mut Self>, key: &QString) {
        let Some(services) = self.app_services() else { return };
        let Some(licensing) = services.licensing.use_cases.clone() else {
            return self.set_error_message(QString::from(NOT_CONFIGURED_NOTE));
        };
        self.as_mut().set_busy(true);
        self.as_mut().set_error_message(QString::default());
        let (key, thread) = (key.to_string(), self.qt_thread());
        services.runtime.spawn(async move {
            let result = licensing.activate.execute(&key, now()).await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_busy(false);
                match result {
                    Ok(status) => controller.show(&status),
                    Err(error) => controller.set_error_message(QString::from(&activation_message(&error))),
                }
            });
        });
    }

    pub fn deactivate(mut self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return };
        let Some(licensing) = services.licensing.use_cases.clone() else { return };
        self.as_mut().set_busy(true);
        self.as_mut().set_error_message(QString::default());
        let thread = self.qt_thread();
        services.runtime.spawn(async move {
            let result = licensing.deactivate.execute().await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_busy(false);
                match result {
                    Ok(()) => controller.show(&LicenseStatus::Free),
                    Err(error) => controller.set_error_message(QString::from(&activation_message(&error))),
                }
            });
        });
    }

    pub fn require(mut self: Pin<&mut Self>, feature: &QString) -> bool {
        let known = ProFeature::from_key(&feature.to_string()).is_some();
        if *self.pro() || !known {
            return true;
        }
        self.as_mut().upgrade_requested(feature.clone());
        false
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }

    fn show(mut self: Pin<&mut Self>, status: &LicenseStatus) {
        let (edition, text, activated) = describe(status);
        self.as_mut().set_pro(status.is_pro());
        self.as_mut().set_edition(QString::from(edition));
        self.as_mut().set_activated(activated);
        self.set_status_text(QString::from(&text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_read_as_plain_sentences() {
        assert_eq!(describe(&LicenseStatus::Free).0, "Free");
        let pro = describe(&LicenseStatus::Pro { licensee: "Ada".into(), license_expiry: None });
        assert_eq!((pro.0, pro.1.as_str(), pro.2), ("Pro", "Licensed to Ada.", true));
        assert!(describe(&LicenseStatus::NeedsConfirmation { licensee: "Ada".into() }).1.contains("Connect"));
        assert_eq!(
            activation_message(&LicenseError::MachineLimit),
            "This license is already in use on its maximum number of computers."
        );
    }
}

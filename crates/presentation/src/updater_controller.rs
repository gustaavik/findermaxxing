//! `UpdaterController`: "Check for Updates…" and automatic checks, in the
//! way each platform's users expect (ADR 0031):
//!
//! - macOS: Sparkle 2 (signed appcast; its own consent prompt and windows);
//! - Linux AppImage: `appimageupdatetool` with the zsync info embedded in
//!   the AppImage, if installed; otherwise the releases page;
//! - Linux Flatpak: Flatpak itself (the software center), nothing in-app.
//!
//! Nothing touches the network unless the user agreed (Sparkle asks on the
//! second launch) or clicked "Check for Updates…".

use std::pin::Pin;
use std::process::Command;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use crate::updater_bridge;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[qenum(UpdaterController)]
    enum UpdateMechanism {
        /// No updater in this build (development, or an unknown packaging).
        Unavailable,
        Sparkle,
        AppImage,
        Flatpak,
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(UpdateMechanism, mechanism)]
        #[qproperty(bool, automatic)]
        #[qproperty(bool, busy)]
        #[qproperty(QString, version)]
        #[qproperty(QString, releases_url)]
        /// What happened last, or how updates arrive here.
        #[qproperty(QString, note)]
        type UpdaterController = super::UpdaterControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        /// Detects how this copy is updated and starts Sparkle on macOS.
        #[qinvokable]
        fn start(self: Pin<&mut UpdaterController>);
        #[qinvokable]
        fn check_now(self: Pin<&mut UpdaterController>);
        #[qinvokable]
        fn set_automatic_checks(self: Pin<&mut UpdaterController>, enabled: bool);
    }

    impl cxx_qt::Threading for UpdaterController {}
}

use qobject::UpdateMechanism;

/// Set by the AppImage runtime to the image's own path.
const APPIMAGE_VARIABLE: &str = "APPIMAGE";
/// Set inside every Flatpak sandbox.
const FLATPAK_VARIABLE: &str = "FLATPAK_ID";
const APPIMAGE_UPDATER: &str = "appimageupdatetool";
/// Where "Download" leads when no updater can do it in place.
const RELEASES_URL: Option<&str> = option_env!("FM_RELEASES_URL");

pub struct UpdaterControllerRust {
    mechanism: UpdateMechanism,
    automatic: bool,
    busy: bool,
    version: QString,
    releases_url: QString,
    note: QString,
}

impl Default for UpdaterControllerRust {
    fn default() -> Self {
        Self {
            mechanism: UpdateMechanism::Unavailable,
            automatic: false,
            busy: false,
            version: QString::from(env!("CARGO_PKG_VERSION")),
            releases_url: QString::from(RELEASES_URL.unwrap_or_default()),
            note: QString::default(),
        }
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// How this copy is updated (the Linux part; macOS asks Sparkle).
fn linux_mechanism() -> (UpdateMechanism, &'static str) {
    if std::env::var_os(FLATPAK_VARIABLE).is_some() {
        (UpdateMechanism::Flatpak, "Updates arrive through Flatpak (your software center or `flatpak update`).")
    } else if std::env::var_os(APPIMAGE_VARIABLE).is_some() {
        if on_path(APPIMAGE_UPDATER) {
            (UpdateMechanism::AppImage, "Updates download only the changed parts of the AppImage.")
        } else {
            (UpdateMechanism::AppImage, "Install AppImageUpdate to update in place, or download the new version.")
        }
    } else {
        (UpdateMechanism::Unavailable, "This copy wasn't installed from a release; it isn't updated automatically.")
    }
}

impl qobject::UpdaterController {
    pub fn start(mut self: Pin<&mut Self>) {
        if cfg!(target_os = "macos") {
            let available = updater_bridge::start_sparkle();
            let mechanism = if available { UpdateMechanism::Sparkle } else { UpdateMechanism::Unavailable };
            self.as_mut().set_mechanism(mechanism);
            self.as_mut().set_automatic(available && updater_bridge::automatic_update_checks());
            let note = if available { "" } else { "Development builds aren't updated automatically." };
            self.set_note(QString::from(note));
        } else {
            let (mechanism, note) = linux_mechanism();
            self.as_mut().set_mechanism(mechanism);
            self.set_note(QString::from(note));
        }
    }

    pub fn check_now(mut self: Pin<&mut Self>) {
        match *self.mechanism() {
            UpdateMechanism::Sparkle => updater_bridge::check_for_updates_now(),
            UpdateMechanism::AppImage if on_path(APPIMAGE_UPDATER) && !*self.busy() => {
                let Some(image) = std::env::var_os(APPIMAGE_VARIABLE) else { return };
                self.as_mut().set_busy(true);
                self.as_mut().set_note(QString::from("Looking for an update…"));
                let thread = self.qt_thread();
                std::thread::spawn(move || {
                    // `-O`: replace the image in place; the next start runs the new version.
                    let outcome = Command::new(APPIMAGE_UPDATER).arg("-O").arg(&image).output();
                    let note = match outcome {
                        Ok(output) if output.status.success() => {
                            "Updated. Quit and reopen FinderMaxxing to use the new version."
                        }
                        Ok(_) => "No update could be installed; the releases page has the latest version.",
                        Err(_) => "AppImageUpdate couldn't be started.",
                    };
                    let _ = thread.queue(move |mut controller| {
                        controller.as_mut().set_busy(false);
                        controller.set_note(QString::from(note));
                    });
                });
            }
            _ => {}
        }
    }

    pub fn set_automatic_checks(mut self: Pin<&mut Self>, enabled: bool) {
        if *self.mechanism() == UpdateMechanism::Sparkle {
            updater_bridge::set_automatic_update_checks(enabled);
            self.as_mut().set_automatic(updater_bridge::automatic_update_checks());
        }
    }
}

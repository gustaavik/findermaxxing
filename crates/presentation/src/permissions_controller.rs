//! `PermissionsController`: reads the permissions of a selection for the
//! Permissions dialog (tri-state per bit when the items differ). Applying a
//! change is a job on the window's `OperationsController` (undoable).

use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::{QString, QStringList};
use domain::{StoragePath, UnixMode, common_bits, octal, symbolic};

use crate::services::AppServices;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;

        include!("presentation/cpp/services_holder.h");
        type ServicesHolder = crate::services_bridge::ServicesHolder;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(*mut ServicesHolder, services)]
        #[qproperty(bool, busy)]
        #[qproperty(bool, editable)]
        #[qproperty(bool, has_folders)]
        /// Nine characters, owner/group/others × read/write/execute:
        /// `1` set on all items, `0` on none, `?` mixed.
        #[qproperty(QString, bits)]
        /// `755` or `rwxr-xr-x` when all items agree, empty otherwise.
        #[qproperty(QString, octal)]
        #[qproperty(QString, symbolic)]
        #[qproperty(QString, error_message)]
        type PermissionsController = super::PermissionsControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn load(self: Pin<&mut PermissionsController>, locations: &QStringList);
        /// `755` → nine `1`/`0` characters, or empty if it isn't a mode.
        #[qinvokable]
        fn bits_of_octal(self: &PermissionsController, text: &QString) -> QString;
    }

    impl cxx_qt::Threading for PermissionsController {}
}

use qobject::ServicesHolder;

const NOT_EDITABLE_NOTE: &str = "Permissions can't be changed on this location.";

pub struct PermissionsControllerRust {
    services: *mut ServicesHolder,
    busy: bool,
    editable: bool,
    has_folders: bool,
    bits: QString,
    octal: QString,
    symbolic: QString,
    error_message: QString,
}

impl Default for PermissionsControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            busy: false,
            editable: false,
            has_folders: false,
            bits: QString::default(),
            octal: QString::default(),
            symbolic: QString::default(),
            error_message: QString::default(),
        }
    }
}

impl qobject::PermissionsController {
    pub fn load(mut self: Pin<&mut Self>, locations: &QStringList) {
        let Some(services) = self.app_services() else { return };
        let items: Vec<StoragePath> =
            locations.iter().filter_map(|l| StoragePath::decode(&l.to_string()).ok()).collect();
        self.as_mut().set_busy(true);
        self.as_mut().set_error_message(QString::default());
        let (use_case, thread) = (services.utilities.get_permissions.clone(), self.qt_thread());
        services.runtime.spawn(async move {
            let view = use_case.execute(&items).await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_busy(false);
                match view {
                    Ok(view) => {
                        controller.as_mut().set_editable(view.editable);
                        controller.as_mut().set_has_folders(view.has_folders);
                        controller.as_mut().show(&view.modes);
                        if !view.editable {
                            controller.set_error_message(QString::from(NOT_EDITABLE_NOTE));
                        }
                    }
                    Err(error) => controller.set_error_message(QString::from(&error.to_string())),
                }
            });
        });
    }

    pub fn bits_of_octal(&self, text: &QString) -> QString {
        domain::parse_octal(&text.to_string()).map_or_else(QString::default, |mode| QString::from(&tri_state(&[mode])))
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }

    fn show(mut self: Pin<&mut Self>, modes: &[UnixMode]) {
        let all_same = modes.windows(2).all(|pair| pair[0] == pair[1]);
        let (octal_text, symbolic_text) = match modes.first() {
            Some(mode) if all_same => (octal(*mode), symbolic(*mode)),
            _ => (String::new(), String::new()),
        };
        self.as_mut().set_bits(QString::from(&tri_state(modes)));
        self.as_mut().set_octal(QString::from(&octal_text));
        self.set_symbolic(QString::from(&symbolic_text));
    }
}

fn tri_state(modes: &[UnixMode]) -> String {
    common_bits(modes)
        .iter()
        .flatten()
        .map(|bit| match bit {
            Some(true) => '1',
            Some(false) => '0',
            None => '?',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_bits_are_question_marks() {
        assert_eq!(tri_state(&[UnixMode::from_mode(0o755)]), "111101101");
        assert_eq!(tri_state(&[UnixMode::from_mode(0o644), UnixMode::from_mode(0o600)]), "110?00?00");
    }
}

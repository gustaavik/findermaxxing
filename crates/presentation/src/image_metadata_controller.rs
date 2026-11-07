//! `ImageMetadataController`: shows an image's EXIF, flagging what gives
//! away where it was taken or who took it. Removing it is a job on the
//! window's `OperationsController` (undoable).

use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use domain::{MetadataField, Sensitivity, StoragePath};
use serde_json::json;

use crate::error_messages::operation_message;
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
        #[qproperty(bool, busy)]
        /// `[{group, tag, value, sensitive}]`, location and identity first.
        #[qproperty(QString, fields_json)]
        #[qproperty(bool, reveals_location)]
        #[qproperty(bool, reveals_identity)]
        #[qproperty(QString, error_message)]
        type ImageMetadataController = super::ImageMetadataControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn load(self: Pin<&mut ImageMetadataController>, location: &QString);
    }

    impl cxx_qt::Threading for ImageMetadataController {}
}

use qobject::ServicesHolder;

pub struct ImageMetadataControllerRust {
    services: *mut ServicesHolder,
    busy: bool,
    fields_json: QString,
    reveals_location: bool,
    reveals_identity: bool,
    error_message: QString,
}

impl Default for ImageMetadataControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            busy: false,
            fields_json: QString::from("[]"),
            reveals_location: false,
            reveals_identity: false,
            error_message: QString::default(),
        }
    }
}

impl qobject::ImageMetadataController {
    pub fn load(mut self: Pin<&mut Self>, location: &QString) {
        let (Some(services), Ok(image)) = (self.app_services(), StoragePath::decode(&location.to_string())) else {
            return;
        };
        self.as_mut().set_busy(true);
        self.as_mut().set_error_message(QString::default());
        self.as_mut().set_fields_json(QString::from("[]"));
        let (use_case, thread) = (services.utilities.read_metadata.clone(), self.qt_thread());
        services.runtime.spawn(async move {
            let fields = use_case.execute(&image).await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_busy(false);
                match fields {
                    Ok(fields) => controller.show(fields),
                    Err(error) => {
                        let message = operation_message(&error).unwrap_or_default();
                        controller.set_error_message(QString::from(&message));
                    }
                }
            });
        });
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }

    fn show(mut self: Pin<&mut Self>, mut fields: Vec<MetadataField>) {
        let rank = |s: Sensitivity| match s {
            Sensitivity::Location => 0,
            Sensitivity::Identity => 1,
            Sensitivity::Other => 2,
        };
        fields.sort_by_key(|f| rank(f.sensitivity));
        let has = |wanted: Sensitivity| fields.iter().any(|f| f.sensitivity == wanted);
        self.as_mut().set_reveals_location(has(Sensitivity::Location));
        self.as_mut().set_reveals_identity(has(Sensitivity::Identity));
        self.set_fields_json(QString::from(&fields_json(&fields)));
    }
}

fn fields_json(fields: &[MetadataField]) -> String {
    let rows: Vec<serde_json::Value> = fields
        .iter()
        .map(|f| {
            json!({
                "group": f.group,
                "tag": f.tag,
                "value": f.value,
                "sensitive": f.sensitivity != Sensitivity::Other,
            })
        })
        .collect();
    serde_json::Value::Array(rows).to_string()
}

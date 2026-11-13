//! `CloudModelController`: the cloud model dialog. Which provider and model,
//! the API key (kept in the keychain, never shown again), and per-folder
//! opt-in with a preview of exactly what would be sent (ADR 0029).

use std::pin::Pin;

use application::ports::{CloudModelConfig, CloudProvider, OrganizeRequest, Secret};
use cxx_qt::Threading;
use cxx_qt_lib::QString;
use domain::StoragePath;

use crate::clock_bridge::query_clock;
use crate::services::AppServices;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("presentation/cpp/services_holder.h");
        type ServicesHolder = crate::services_bridge::ServicesHolder;
    }

    #[qenum(CloudModelController)]
    enum Provider {
        Anthropic,
        OpenAiCompatible,
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(*mut ServicesHolder, services)]
        #[qproperty(bool, configured)]
        #[qproperty(bool, has_key)]
        #[qproperty(Provider, provider)]
        #[qproperty(QString, base_url)]
        #[qproperty(QString, model)]
        /// Whether the folder last asked about is opted in.
        #[qproperty(bool, opted_in)]
        /// What would be sent for that folder, as readable lines.
        #[qproperty(QString, preview_text)]
        #[qproperty(i32, preview_items)]
        /// The host the summaries would go to.
        #[qproperty(QString, recipient)]
        #[qproperty(bool, busy)]
        #[qproperty(QString, error_message)]
        type CloudModelController = super::CloudModelControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        /// Loads the settings and whether `location` is opted in.
        #[qinvokable]
        fn refresh(self: Pin<&mut CloudModelController>, location: &QString);
        /// An empty `key` keeps the stored one.
        #[qinvokable]
        fn save(
            self: Pin<&mut CloudModelController>,
            provider: Provider,
            base_url: &QString,
            model: &QString,
            key: &QString,
        );
        #[qinvokable]
        fn forget(self: Pin<&mut CloudModelController>);
        #[qinvokable]
        fn set_opt_in(self: Pin<&mut CloudModelController>, location: &QString, allowed: bool);
        #[qinvokable]
        fn load_preview(self: Pin<&mut CloudModelController>, location: &QString);
        /// The usual base URL and a model for a provider.
        #[qinvokable]
        fn default_base_url(self: &CloudModelController, provider: Provider) -> QString;
        #[qinvokable]
        fn default_model(self: &CloudModelController, provider: Provider) -> QString;

        #[qsignal]
        fn changed(self: Pin<&mut CloudModelController>);
    }

    impl cxx_qt::Threading for CloudModelController {}
}

use qobject::{Provider, ServicesHolder};

const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_MODEL: &str = "claude-opus-5-5";
const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
/// Lines of the preview before it says "and N more".
const PREVIEW_LINES: usize = 40;

// The bools are Q_PROPERTY storage, which cxx-qt requires as plain fields.
#[allow(clippy::struct_excessive_bools, reason = "mirrors Qt properties one-to-one")]
pub struct CloudModelControllerRust {
    services: *mut ServicesHolder,
    configured: bool,
    has_key: bool,
    provider: Provider,
    base_url: QString,
    model: QString,
    opted_in: bool,
    preview_text: QString,
    preview_items: i32,
    recipient: QString,
    busy: bool,
    error_message: QString,
}

impl Default for CloudModelControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            configured: false,
            has_key: false,
            provider: Provider::Anthropic,
            base_url: QString::from(ANTHROPIC_BASE_URL),
            model: QString::from(ANTHROPIC_MODEL),
            opted_in: false,
            preview_text: QString::default(),
            preview_items: 0,
            recipient: QString::default(),
            busy: false,
            error_message: QString::default(),
        }
    }
}

fn provider_of(provider: Provider) -> CloudProvider {
    if provider == Provider::OpenAiCompatible { CloudProvider::OpenAiCompatible } else { CloudProvider::Anthropic }
}

fn host_of(base_url: &str) -> String {
    url_host(base_url).unwrap_or_else(|| base_url.to_owned())
}

/// `https://api.anthropic.com/v1` → `api.anthropic.com`.
fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?']).next().filter(|h| !h.is_empty()).map(str::to_owned)
}

impl qobject::CloudModelController {
    pub fn refresh(mut self: Pin<&mut Self>, location: &QString) {
        let Some(services) = self.app_services() else { return };
        let folder = StoragePath::decode(&location.to_string()).ok();
        self.as_mut().set_busy(true);
        let (setup, thread) = (services.organize.cloud.clone(), self.qt_thread());
        services.runtime.spawn(async move {
            let status = setup.status().await;
            let opted_in = match &folder {
                Some(folder) => setup.is_opted_in(folder).await.unwrap_or(false),
                None => false,
            };
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_busy(false);
                match status {
                    Ok(status) => {
                        controller.as_mut().set_has_key(status.has_key);
                        controller.as_mut().set_configured(status.settings.is_some() && status.has_key);
                        if let Some(settings) = status.settings {
                            let provider = match settings.config.provider {
                                CloudProvider::Anthropic => Provider::Anthropic,
                                CloudProvider::OpenAiCompatible => Provider::OpenAiCompatible,
                            };
                            controller.as_mut().set_provider(provider);
                            controller.as_mut().set_base_url(QString::from(&settings.config.base_url));
                            controller.as_mut().set_model(QString::from(&settings.config.model));
                        }
                        let recipient = host_of(&controller.base_url().to_string());
                        controller.as_mut().set_recipient(QString::from(&recipient));
                        controller.as_mut().set_opted_in(opted_in);
                    }
                    Err(error) => controller.as_mut().set_error_message(QString::from(&error.to_string())),
                }
                controller.changed();
            });
        });
    }

    pub fn save(mut self: Pin<&mut Self>, provider: Provider, base_url: &QString, model: &QString, key: &QString) {
        let Some(services) = self.app_services() else { return };
        let config = CloudModelConfig {
            provider: provider_of(provider),
            base_url: base_url.to_string().trim().to_owned(),
            model: model.to_string().trim().to_owned(),
        };
        if config.base_url.is_empty() || config.model.is_empty() {
            self.set_error_message(QString::from("Enter the address and the model."));
            return;
        }
        let key = Some(key.to_string().trim().to_owned()).filter(|k| !k.is_empty()).map(Secret::new);
        self.as_mut().set_error_message(QString::default());
        self.as_mut().set_busy(true);
        let (setup, thread) = (services.organize.cloud.clone(), self.qt_thread());
        services.runtime.spawn(async move {
            let saved = setup.configure(config, key).await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_busy(false);
                match saved {
                    Ok(()) => controller.refresh(&QString::default()),
                    Err(error) => controller.set_error_message(QString::from(&error.to_string())),
                }
            });
        });
    }

    pub fn forget(mut self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return };
        self.as_mut().set_busy(true);
        let (setup, thread) = (services.organize.cloud.clone(), self.qt_thread());
        services.runtime.spawn(async move {
            let forgotten = setup.forget().await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_busy(false);
                match forgotten {
                    Ok(()) => {
                        controller.as_mut().set_configured(false);
                        controller.as_mut().set_has_key(false);
                        controller.as_mut().set_opted_in(false);
                        controller.changed();
                    }
                    Err(error) => controller.set_error_message(QString::from(&error.to_string())),
                }
            });
        });
    }

    pub fn set_opt_in(mut self: Pin<&mut Self>, location: &QString, allowed: bool) {
        let (Some(services), Ok(folder)) = (self.app_services(), StoragePath::decode(&location.to_string())) else {
            return;
        };
        self.as_mut().set_busy(true);
        let (setup, thread, location) = (services.organize.cloud.clone(), self.qt_thread(), location.clone());
        services.runtime.spawn(async move {
            let result = setup.set_opt_in(&folder, allowed).await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_busy(false);
                match result {
                    Ok(()) => controller.refresh(&location),
                    Err(error) => controller.set_error_message(QString::from(&error.to_string())),
                }
            });
        });
    }

    pub fn load_preview(mut self: Pin<&mut Self>, location: &QString) {
        let (Some(services), Ok(folder)) = (self.app_services(), StoragePath::decode(&location.to_string())) else {
            return;
        };
        self.as_mut().set_busy(true);
        self.as_mut().set_preview_text(QString::default());
        let (preview, thread) = (services.organize.preview.clone(), self.qt_thread());
        services.runtime.spawn(async move {
            let request = preview.execute(&folder, query_clock()).await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_busy(false);
                match request {
                    Ok(request) => {
                        controller.as_mut().set_preview_items(i32::try_from(request.items.len()).unwrap_or(i32::MAX));
                        controller.set_preview_text(QString::from(&preview_text(&request)));
                    }
                    Err(error) => controller.set_error_message(QString::from(&error.to_string())),
                }
            });
        });
    }

    pub fn default_base_url(&self, provider: Provider) -> QString {
        QString::from(if provider == Provider::OpenAiCompatible { OPENAI_BASE_URL } else { ANTHROPIC_BASE_URL })
    }

    pub fn default_model(&self, provider: Provider) -> QString {
        QString::from(if provider == Provider::OpenAiCompatible { "" } else { ANTHROPIC_MODEL })
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }
}

/// The request as the user reads it: one line per item, excerpts quoted.
fn preview_text(request: &OrganizeRequest) -> String {
    let mut lines = vec![format!("Folder name: {}", request.folder_name)];
    for item in request.items.iter().take(PREVIEW_LINES) {
        let mut line = format!("• {} ({}", item.name, item.kind);
        if let Some(modified) = &item.modified {
            line.push_str(&format!(", {modified}"));
        }
        line.push(')');
        if let Some(excerpt) = &item.excerpt {
            line.push_str(&format!("\n    “{excerpt}”"));
        }
        lines.push(line);
    }
    if request.items.len() > PREVIEW_LINES {
        lines.push(format!("… and {} more items, described the same way.", request.items.len() - PREVIEW_LINES));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::ports::ItemSummary;

    #[test]
    fn previews_list_items_with_their_excerpts_and_hosts_are_shown_bare() {
        let request = OrganizeRequest {
            folder_name: "Docs".into(),
            items: vec![ItemSummary {
                id: 0,
                name: "a.pdf".into(),
                kind: "pdf".into(),
                size: Some(3),
                modified: Some("2026-03-14".into()),
                excerpt: Some("Lease".into()),
            }],
        };
        assert_eq!(preview_text(&request), "Folder name: Docs\n• a.pdf (pdf, 2026-03-14)\n    “Lease”");
        assert_eq!(host_of("https://api.anthropic.com"), "api.anthropic.com");
        assert_eq!(host_of("http://localhost:11434/v1"), "localhost:11434");
    }
}

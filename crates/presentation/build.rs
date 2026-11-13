use cxx_qt_build::{CxxQtBuilder, QmlModule};

const QML_MODULE_URI: &str = "FinderMaxxing";

/// Every QML file of the module.
const QML_FILES: &[&str] = &[
    "qml/Main.qml",
    "qml/FolderView.qml",
    "qml/FileIcon.qml",
    "qml/JobsPanel.qml",
    "qml/ConflictDialog.qml",
    "qml/PreviewPane.qml",
    "qml/BulkRenameDialog.qml",
    "qml/SearchBar.qml",
    "qml/Sidebar.qml",
    "qml/AddAccountDialog.qml",
    "qml/ConnectServerDialog.qml",
    "qml/AiSearchDialog.qml",
    "qml/CostDialog.qml",
    "qml/InfoDialog.qml",
    "qml/ChecksumDialog.qml",
    "qml/DuplicatesWindow.qml",
    "qml/DiskUsageWindow.qml",
    "qml/ImageToolsDialog.qml",
    "qml/PermissionsDialog.qml",
    "qml/CompressDialog.qml",
    "qml/SecureDeleteDialog.qml",
    "qml/TerminalPane.qml",
    "qml/OrganizeWindow.qml",
    "qml/CloudModelDialog.qml",
];

/// Rust files with `#[cxx::bridge]` or `#[cxx_qt::bridge]` modules.
const BRIDGE_FILES: &[&str] = &[
    "src/rows_bridge.rs",
    "src/services_bridge.rs",
    "src/clipboard_bridge.rs",
    "src/input_bridge.rs",
    "src/clock_bridge.rs",
    "src/folder_view_controller.rs",
    "src/operations_controller.rs",
    "src/preview_controller.rs",
    "src/quick_look_bridge.rs",
    "src/bulk_rename_controller.rs",
    "src/sidebar_controller.rs",
    "src/item_details_controller.rs",
    "src/ai_search_controller.rs",
    "src/checksum_controller.rs",
    "src/duplicates_controller.rs",
    "src/disk_usage_controller.rs",
    "src/image_metadata_controller.rs",
    "src/permissions_controller.rs",
    "src/terminal_controller.rs",
    "src/organize_controller.rs",
    "src/cloud_model_controller.rs",
];

/// C++ shims; headers with `Q_OBJECT` are listed so they get moc'd.
const CPP_FILES: &[&str] = &[
    "cpp/entry_list_model.h",
    "cpp/entry_list_model.cpp",
    "cpp/services_holder.h",
    "cpp/services_holder.cpp",
    "cpp/job_list_model.h",
    "cpp/job_list_model.cpp",
    "cpp/clipboard.cpp",
    "cpp/input.cpp",
    "cpp/clock.cpp",
    "cpp/sidebar_model.h",
    "cpp/sidebar_model.cpp",
    "cpp/terminal_view.h",
    "cpp/terminal_view.cpp",
    "cpp/bootstrap.cpp",
];

fn main() {
    macos_sdk_shims::provide_missing_frameworks();

    let module = QmlModule::new(QML_MODULE_URI).qml_files(QML_FILES.iter().copied());

    // SAFETY: only adds a language-standard flag; no other cc settings are touched.
    let builder = unsafe {
        CxxQtBuilder::new_qml_module(module).cc_builder(|cc| {
            cc.flag_if_supported("-std=c++17");
        })
    };

    builder
        .qt_module("Gui")
        .qt_module("Quick")
        .files(BRIDGE_FILES.iter().copied())
        .cpp_files(CPP_FILES.iter().copied().chain([quick_look_source()]))
        .build();
}

/// Quick Look exists only on macOS; elsewhere a stub answers "unavailable".
fn quick_look_source() -> &'static str {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo::rustc-link-lib=framework=Quartz");
        println!("cargo::rustc-link-lib=framework=AppKit");
        "cpp/quick_look_mac.mm"
    } else {
        "cpp/quick_look_other.cpp"
    }
}

/// Qt 6.8.3's QtGui metadata asks the linker for `-framework AGL`. macOS 26
/// SDKs dropped AGL's link stub although the framework still ships in the OS
/// (QtGui itself loads it). When the active SDK lacks it, generate a
/// symbol-less text stub so linking succeeds; at runtime the real system
/// framework is used. Remove once Qt no longer references AGL.
mod macos_sdk_shims {
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::{env, fs};

    const AGL_STUB: &str = "--- !tapi-tbd\n\
        tbd-version: 4\n\
        targets: [ x86_64-macos, arm64-macos ]\n\
        install-name: '/System/Library/Frameworks/AGL.framework/Versions/A/AGL'\n\
        current-version: 1\n\
        compatibility-version: 1\n\
        ...\n";

    pub fn provide_missing_frameworks() {
        if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
            return;
        }
        let Some(sdk) = sdk_path() else { return };
        if sdk.join("System/Library/Frameworks/AGL.framework").exists() {
            return;
        }
        let Some(out_dir) = env::var_os("OUT_DIR").map(PathBuf::from) else { return };
        let shim_root = out_dir.join("sdk-shims");
        let framework = shim_root.join("AGL.framework");
        if fs::create_dir_all(&framework).and_then(|()| fs::write(framework.join("AGL.tbd"), AGL_STUB)).is_ok() {
            println!("cargo::rustc-link-search=framework={}", shim_root.display());
        }
    }

    fn sdk_path() -> Option<PathBuf> {
        let output = Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]).output().ok()?;
        let path = String::from_utf8(output.stdout).ok()?;
        Some(Path::new(path.trim()).to_path_buf())
    }
}

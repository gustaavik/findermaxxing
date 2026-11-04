use cxx_qt_build::{CxxQtBuilder, QmlModule};

const QML_MODULE_URI: &str = "FinderMaxxing";

fn main() {
    macos_sdk_shims::provide_missing_frameworks();

    let module = QmlModule::new(QML_MODULE_URI).qml_files(["qml/Main.qml", "qml/FolderView.qml", "qml/FileIcon.qml"]);

    // SAFETY: only adds a language-standard flag; no other cc settings are touched.
    let builder = unsafe {
        CxxQtBuilder::new_qml_module(module).cc_builder(|cc| {
            cc.flag_if_supported("-std=c++17");
        })
    };

    builder
        .qt_module("Gui")
        .qt_module("Quick")
        .files(["src/rows_bridge.rs", "src/services_bridge.rs", "src/folder_view_controller.rs"])
        .cpp_files([
            "cpp/entry_list_model.h",
            "cpp/entry_list_model.cpp",
            "cpp/services_holder.h",
            "cpp/services_holder.cpp",
            "cpp/bootstrap.cpp",
        ])
        .build();
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

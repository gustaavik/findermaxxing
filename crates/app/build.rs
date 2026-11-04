//! Lets development builds find the dynamically linked Qt libraries (LGPL
//! requires dynamic linking). Release bundles get their own rpath from
//! macdeployqt / linuxdeploy, which takes precedence over this one.

use std::env;
use std::process::Command;

const QMAKE_VARIABLE: &str = "QMAKE";
const DEFAULT_QMAKE: &str = "qmake";

fn main() {
    println!("cargo::rerun-if-env-changed={QMAKE_VARIABLE}");
    let qmake = env::var(QMAKE_VARIABLE).unwrap_or_else(|_| DEFAULT_QMAKE.to_owned());
    let Some(qt_libs) = query_qmake(&qmake, "QT_INSTALL_LIBS") else {
        println!("cargo::warning=could not query {qmake}; the binary may not find Qt at runtime");
        return;
    };
    println!("cargo::rustc-link-arg-bins=-Wl,-rpath,{qt_libs}");
}

fn query_qmake(qmake: &str, variable: &str) -> Option<String> {
    let output = Command::new(qmake).args(["-query", variable]).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

//! `cargo xtask package-macos`    — FinderMaxxing.app and a DMG (ADR 0031);
//!   `--universal` for Apple silicon + Intel, `--sign`, `--notarize`.
//! `cargo xtask package-appimage` — a self-updating AppImage (Linux).
//!
//! Both build in release mode with whatever `FM_*` variables are set (the
//! license server, store and releases URLs are compiled in, ADR 0030), and
//! write to `target/package/`.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const APP_NAME: &str = "FinderMaxxing";
const APP_ID: &str = "dev.findermaxxing.FinderMaxxing";
const BINARY: &str = "findermaxxing";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const OUT_DIR: &str = "target/package";
const QML_DIR: &str = "crates/presentation/qml";
const LICENSE_REPORT: &str = "target/third-party-licenses.html";
const BUNDLED_LICENSES: &str = "packaging/licenses";
const MINIMUM_MACOS: &str = "14.0";

/// Sparkle is downloaded once, pinned by checksum (it's code we ship).
const SPARKLE_VERSION: &str = "2.10.1";
const SPARKLE_SHA256: &str = "1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27";
const AD_HOC: &str = "-";
/// Plugins macdeployqt adds that the app never loads (SQLite goes through
/// rusqlite; the ODBC and PostgreSQL drivers' libraries aren't even there).
const UNUSED_QT_PLUGINS: &[&str] = &["sqldrivers"];

pub type Outcome = Result<(), String>;

#[derive(Debug, Default)]
struct MacOptions {
    identity: Option<String>,
    notary_profile: Option<String>,
    /// One binary for Apple silicon and Intel (Qt's own frameworks are universal).
    universal: bool,
}

/// The Rust targets of a universal build.
const MAC_TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
/// Qt 6.8.3's `qyieldcpu.h` calls `__yield` without including `<arm_acle.h>`,
/// which Xcode 16.3+ rejects on arm64.
const ARM_QT_WORKAROUND: (&str, &str) = ("CXXFLAGS_aarch64_apple_darwin", "-include arm_acle.h");

fn parse_mac_options(args: &[String]) -> Result<MacOptions, String> {
    let mut options = MacOptions::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--sign" => options.identity = Some(value()?),
            "--notarize" => options.notary_profile = Some(value()?),
            "--universal" => options.universal = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    if options.notary_profile.is_some() && options.identity.is_none() {
        return Err("--notarize needs --sign with a Developer ID identity".into());
    }
    Ok(options)
}

fn run(program: &str, args: &[&str]) -> Outcome {
    run_command(Command::new(program).args(args))
}

fn run_command(command: &mut Command) -> Outcome {
    let shown = format!("{command:?}");
    eprintln!("==> {shown}");
    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("{shown} failed ({status})")),
        Err(error) => Err(format!("could not run {shown}: {error}")),
    }
}

fn output(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program).args(args).output().map_err(|e| format!("could not run {program}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{program} {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn io_error(context: &str) -> impl Fn(io::Error) -> String + '_ {
    move |error| format!("{context}: {error}")
}

fn qt_bin(tool: &str) -> Result<String, String> {
    let qmake = env::var("QMAKE").map_err(|_| "set QMAKE to the Qt 6.8 qmake (see README)".to_owned())?;
    Ok(output(&qmake, &["-query", "QT_INSTALL_BINS"])? + "/" + tool)
}

/// Builds the app (for `target`, or the host) and returns the binary.
fn release_build(target: Option<&str>) -> Result<PathBuf, String> {
    let mut command = Command::new("cargo");
    command.args(["build", "--release", "--locked", "-p", "app"]);
    if let Some(target) = target {
        command.args(["--target", target]);
    }
    if cfg!(target_os = "macos") {
        // Otherwise C++ objects target the SDK's macOS, not the minimum we promise.
        command.env("MACOSX_DEPLOYMENT_TARGET", MINIMUM_MACOS);
        if env::var_os(ARM_QT_WORKAROUND.0).is_none() {
            command.env(ARM_QT_WORKAROUND.0, ARM_QT_WORKAROUND.1);
        }
    }
    run_command(&mut command)?;
    Ok(target
        .map_or_else(|| PathBuf::from("target/release"), |t| Path::new("target").join(t).join("release"))
        .join(BINARY))
}

/// Shipping requires the third-party notices (needs cargo-about).
fn license_report() -> Outcome {
    run("cargo", &["about", "generate", "--workspace", "--output-file", LICENSE_REPORT, "about.hbs"])
}

fn mac_binary(universal: bool) -> Result<PathBuf, String> {
    if !universal {
        return release_build(None);
    }
    let mut slices = Vec::new();
    for target in MAC_TARGETS {
        slices.push(release_build(Some(target))?);
    }
    let fat = PathBuf::from("target/universal");
    fs::create_dir_all(&fat).map_err(io_error("creating target/universal"))?;
    let fat = fat.join(BINARY);
    let mut command = Command::new("lipo");
    command.arg("-create").args(&slices).arg("-output").arg(&fat);
    run_command(&mut command)?;
    Ok(fat)
}

fn fresh_dir(path: &Path) -> Outcome {
    if path.exists() {
        fs::remove_dir_all(path).map_err(io_error("clearing the output"))?;
    }
    fs::create_dir_all(path).map_err(io_error("creating the output"))
}

fn copy_licenses(into: &Path) -> Outcome {
    fs::create_dir_all(into).map_err(io_error("creating Licenses"))?;
    fs::copy(LICENSE_REPORT, into.join("third-party-licenses.html")).map_err(io_error(LICENSE_REPORT))?;
    for entry in fs::read_dir(BUNDLED_LICENSES).map_err(io_error(BUNDLED_LICENSES))? {
        let path = entry.map_err(io_error(BUNDLED_LICENSES))?.path();
        fs::copy(&path, into.join(path.file_name().unwrap_or_default())).map_err(io_error("copying a license"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- macOS ---

pub fn macos(args: &[String]) -> Outcome {
    let options = parse_mac_options(args)?;
    let out = PathBuf::from(OUT_DIR);
    let app = out.join(format!("{APP_NAME}.app"));
    let binary = mac_binary(options.universal)?;
    license_report()?;
    let sparkle = sparkle_framework(&out.join("cache"))?;

    fresh_dir(&app)?;
    let contents = app.join("Contents");
    for dir in ["MacOS", "Resources", "Frameworks"] {
        fs::create_dir_all(contents.join(dir)).map_err(io_error("creating the bundle"))?;
    }
    fs::copy(&binary, contents.join("MacOS").join(BINARY)).map_err(io_error("copying the binary"))?;
    fs::copy("packaging/macos/FinderMaxxing.icns", contents.join("Resources/FinderMaxxing.icns"))
        .map_err(io_error("copying the icon"))?;
    fs::write(contents.join("Info.plist"), info_plist()).map_err(io_error("writing Info.plist"))?;
    let licenses = contents.join("Resources/Licenses");
    copy_licenses(&licenses)?;
    fs::copy(sparkle.with_file_name("LICENSE"), licenses.join("Sparkle-LICENSE.txt"))
        .map_err(io_error("copying Sparkle's license"))?;

    let bundle = app.to_str().ok_or("non-UTF-8 output path")?;
    run(&qt_bin("macdeployqt")?, &[bundle, &format!("-qmldir={QML_DIR}"), "-verbose=1"])?;
    for unused in UNUSED_QT_PLUGINS {
        let _ = fs::remove_dir_all(contents.join("PlugIns").join(unused));
    }
    let frameworks = contents.join("Frameworks").join("Sparkle.framework");
    run("ditto", &[path_str(&sparkle)?, path_str(&frameworks)?])?;

    let identity = options.identity.as_deref().unwrap_or(AD_HOC);
    sign_bundle(&app, identity)?;
    run("codesign", &["--verify", "--deep", "--strict", "--verbose=2", bundle])?;

    let dmg = out.join(format!("{APP_NAME}-{VERSION}.dmg"));
    make_dmg(&app, &dmg)?;
    let dmg_path = path_str(&dmg)?;
    if identity != AD_HOC {
        run("codesign", &["--force", "--timestamp", "--sign", identity, dmg_path])?;
    }
    if let Some(profile) = &options.notary_profile {
        run("xcrun", &["notarytool", "submit", dmg_path, "--keychain-profile", profile, "--wait"])?;
        run("xcrun", &["stapler", "staple", dmg_path])?;
    }
    eprintln!("Packaged {} and {}", app.display(), dmg.display());
    if identity == AD_HOC {
        eprintln!("note: signed ad hoc; Gatekeeper will refuse it on other Macs (use --sign and --notarize)");
    }
    Ok(())
}

fn path_str(path: &Path) -> Result<&str, String> {
    path.to_str().ok_or_else(|| format!("non-UTF-8 path {}", path.display()))
}

/// The plist keys Sparkle needs appear only if the release says where its
/// appcast is; without them the updater stays off (development builds).
fn info_plist() -> String {
    let mut extra = String::new();
    if let (Ok(feed), Ok(key)) = (env::var("FM_APPCAST_URL"), env::var("FM_SPARKLE_PUBLIC_KEY")) {
        extra = format!(
            "\t<key>SUFeedURL</key>\n\t<string>{}</string>\n\t<key>SUPublicEDKey</key>\n\t<string>{}</string>\n",
            xml_escape(&feed),
            xml_escape(&key)
        );
    }
    let folder_reason = "FinderMaxxing shows and manages the files you open in it.";
    let keys = [
        ("CFBundleName", APP_NAME),
        ("CFBundleDisplayName", APP_NAME),
        ("CFBundleIdentifier", APP_ID),
        ("CFBundleExecutable", BINARY),
        ("CFBundlePackageType", "APPL"),
        ("CFBundleShortVersionString", VERSION),
        ("CFBundleVersion", VERSION),
        ("CFBundleIconFile", "FinderMaxxing"),
        ("LSMinimumSystemVersion", MINIMUM_MACOS),
        ("LSApplicationCategoryType", "public.app-category.productivity"),
        ("NSDesktopFolderUsageDescription", folder_reason),
        ("NSDocumentsFolderUsageDescription", folder_reason),
        ("NSDownloadsFolderUsageDescription", folder_reason),
        ("NSRemovableVolumesUsageDescription", folder_reason),
        ("NSNetworkVolumesUsageDescription", folder_reason),
    ];
    let strings: String =
        keys.iter().map(|(k, v)| format!("\t<key>{k}</key>\n\t<string>{}</string>\n", xml_escape(v))).collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n{strings}\
         \t<key>NSHighResolutionCapable</key>\n\t<true/>\n\t<key>NSPrincipalClass</key>\n\t<string>NSApplication</string>\n\
         {extra}</dict>\n</plist>\n"
    )
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `Sparkle.framework` from the pinned release, downloaded into `cache`.
fn sparkle_framework(cache: &Path) -> Result<PathBuf, String> {
    let extracted = cache.join(format!("sparkle-{SPARKLE_VERSION}"));
    let framework = extracted.join("Sparkle.framework");
    if framework.exists() {
        return Ok(framework);
    }
    fs::create_dir_all(&extracted).map_err(io_error("creating the cache"))?;
    let archive = cache.join(format!("Sparkle-{SPARKLE_VERSION}.tar.xz"));
    if !archive.exists() {
        let url = format!(
            "https://github.com/sparkle-project/Sparkle/releases/download/{SPARKLE_VERSION}/Sparkle-{SPARKLE_VERSION}.tar.xz"
        );
        run("curl", &["--fail", "--location", "--silent", "--show-error", "--output", path_str(&archive)?, &url])?;
    }
    let sum = output("shasum", &["-a", "256", path_str(&archive)?])?;
    if sum.split_whitespace().next() != Some(SPARKLE_SHA256) {
        let _ = fs::remove_file(&archive);
        return Err(format!("{} doesn't match the pinned checksum", archive.display()));
    }
    run("tar", &["-xJf", path_str(&archive)?, "-C", path_str(&extracted)?])?;
    Ok(framework)
}

/// Everything that holds code, innermost first, so each signature covers
/// already-signed contents (Apple's "inside out" rule). Symlinks are
/// skipped: framework `Versions/Current` aliases would be signed twice.
fn code_items(dir: &Path, depth: usize, found: &mut Vec<(usize, PathBuf)>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if kind.is_dir() {
            code_items(&path, depth + 1, found)?;
            if [".framework", ".app", ".xpc"].iter().any(|ext| name.ends_with(ext)) {
                found.push((depth, path));
            }
        } else if name.ends_with(".dylib") || name.ends_with(".so") || name == "Autoupdate" {
            found.push((depth, path));
        }
    }
    Ok(())
}

fn sign_bundle(app: &Path, identity: &str) -> Outcome {
    let mut items = Vec::new();
    code_items(&app.join("Contents"), 0, &mut items).map_err(io_error("listing the bundle"))?;
    items.sort_by_key(|item| std::cmp::Reverse(item.0));
    let timestamp = if identity == AD_HOC { "--timestamp=none" } else { "--timestamp" };
    let base = ["--force", "--options", "runtime", timestamp, "--sign", identity];
    for (_, item) in &items {
        // Sparkle's helpers carry their own entitlements; keep them.
        let mut args: Vec<&str> = base.to_vec();
        args.extend(["--preserve-metadata=entitlements", path_str(item)?]);
        run("codesign", &args)?;
    }
    let mut args: Vec<&str> = base.to_vec();
    args.extend(["--entitlements", "packaging/macos/FinderMaxxing.entitlements", path_str(app)?]);
    run("codesign", &args)
}

fn make_dmg(app: &Path, dmg: &Path) -> Outcome {
    let staging = PathBuf::from(OUT_DIR).join("dmg");
    fresh_dir(&staging)?;
    run("ditto", &[path_str(app)?, path_str(&staging.join(format!("{APP_NAME}.app")))?])?;
    std::os::unix::fs::symlink("/Applications", staging.join("Applications"))
        .map_err(io_error("linking Applications"))?;
    let _ = fs::remove_file(dmg);
    let result = run(
        "hdiutil",
        &["create", "-volname", APP_NAME, "-srcfolder", path_str(&staging)?, "-format", "UDZO", path_str(dmg)?],
    );
    let _ = fs::remove_dir_all(&staging);
    result
}

// ---------------------------------------------------------------- Linux ---

/// `owner/repo` of the public releases repository, for in-place updates.
const RELEASES_REPO_VARIABLE: &str = "FM_RELEASES_REPO";

pub fn appimage() -> Outcome {
    let arch = env::consts::ARCH;
    let out = PathBuf::from(OUT_DIR);
    let appdir = out.join("AppDir");
    let binary = release_build(None)?;
    license_report()?;
    fresh_dir(&appdir)?;
    let share = appdir.join("usr/share");
    copy_licenses(&share.join("doc").join(BINARY))?;
    fs::create_dir_all(share.join("metainfo")).map_err(io_error("creating AppDir"))?;
    let metainfo = format!("{APP_ID}.metainfo.xml");
    fs::copy(format!("packaging/linux/{metainfo}"), share.join("metainfo").join(&metainfo))
        .map_err(io_error("copying metainfo"))?;
    // linuxdeploy names the icon after its file; the .desktop file says `Icon=APP_ID`.
    let icon = out.join(format!("{APP_ID}.png"));
    fs::copy("packaging/linux/findermaxxing-256.png", &icon).map_err(io_error("copying the icon"))?;

    let image = format!("{APP_NAME}-{VERSION}-{arch}.AppImage");
    let mut command = Command::new(env::var("LINUXDEPLOY").unwrap_or_else(|_| "linuxdeploy".to_owned()));
    command
        .args(["--appdir", path_str(&appdir)?])
        .args(["--executable", path_str(&binary)?])
        .args(["--desktop-file", &format!("packaging/linux/{APP_ID}.desktop")])
        .args(["--icon-file", path_str(&icon)?])
        .args(["--plugin", "qt", "--output", "appimage"])
        .env("QML_SOURCES_PATHS", QML_DIR)
        .env("EXTRA_PLATFORM_PLUGINS", "libqwayland-egl.so;libqwayland-generic.so")
        .env("LDAI_OUTPUT", &image)
        .env("OUTPUT", &image);
    if let Ok(repo) = env::var(RELEASES_REPO_VARIABLE) {
        let (owner, name) = repo.split_once('/').ok_or(format!("{RELEASES_REPO_VARIABLE} must be owner/repo"))?;
        // AppImageUpdate reads this to fetch only the changed blocks (zsync).
        let info = format!("gh-releases-zsync|{owner}|{name}|latest|{APP_NAME}-*-{arch}.AppImage.zsync");
        command.env("LDAI_UPDATE_INFORMATION", &info).env("UPDATE_INFORMATION", &info);
    } else {
        eprintln!("note: {RELEASES_REPO_VARIABLE} not set; the AppImage won't update in place");
    }
    run_command(&mut command)?;
    eprintln!("Packaged {image}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notarizing_needs_a_real_identity() {
        let args = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(parse_mac_options(&args(&["--notarize", "fm"])).is_err());
        let options = parse_mac_options(&args(&["--sign", "Developer ID", "--notarize", "fm"])).unwrap();
        assert_eq!(options.notary_profile.as_deref(), Some("fm"));
        assert!(parse_mac_options(&args(&["--sign"])).is_err());
    }

    #[test]
    fn the_plist_names_the_binary_and_escapes_values() {
        let plist = info_plist();
        assert!(plist.contains("<key>CFBundleExecutable</key>\n\t<string>findermaxxing</string>"));
        assert_eq!(xml_escape("a&b<c>"), "a&amp;b&lt;c&gt;");
    }
}

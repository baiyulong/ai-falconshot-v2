use cxx_qt_build::{CxxQtBuilder, QmlModule};
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    force_utf8_sources();

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR");
    let shader_qrc = bake_shaders(Path::new(&out_dir));
    embed_icon(Path::new(&out_dir));

    // The recipe M0 proved, applied for real: one QML module holding the twelve
    // documents, the three cxx-qt bridges, and the thin C++ shim nothing else can
    // be (QQuickImageProvider, QWindow::winId, the Win32 extended styles).
    //
    // A document that is not listed here is not in the binary: `main.qml` referencing
    // it is a `module "dev.falconshot" does not have a type named "HintWindow"` at
    // load time, which the engine reports through the message handler and `--probe`
    // fails on - so the registration is checked by a run rather than by eye.
    CxxQtBuilder::new_qml_module(
        QmlModule::new("dev.falconshot")
            .qml_file("qml/main.qml")
            .qml_file("qml/PinWindow.qml")
            .qml_file("qml/CaptureMask.qml")
            .qml_file("qml/AnnotationToolbar.qml")
            .qml_file("qml/HintWindow.qml")
            .qml_file("qml/SettingsWindow.qml")
            .qml_file("qml/CustomCaptureWindow.qml")
            .qml_file("qml/OcrPanel.qml")
            .qml_file("qml/CodesPanel.qml")
            .qml_file("qml/HistoryWindow.qml")
            .qml_file("qml/GroupWindow.qml")
            .qml_file("qml/UiFont.qml"),
    )
    .files(["src/session.rs", "src/pin_view.rs", "src/mask_view.rs"])
    .include_dir("cpp")
    .cpp_files(["cpp/pin_shim.cpp", "cpp/pin_shim.h"])
    .qt_module("Gui")
    .qt_module("Quick")
    .qt_module("Qml")
    .qrc(shader_qrc)
    .build();

    // A .cpp_file shim that calls Win32 gets no import lib for free under a
    // Cargo-led build (CMake's target_link_libraries would have handled it).
    println!("cargo:rustc-link-lib=user32");
}

/// Compile every generated C++ as UTF-8, whatever the machine's code page says.
///
/// `qmlcachegen` writes each QML string literal into the C++ verbatim, and cc-rs
/// (unlike Qt's own CMake) passes no charset flag, so `cl.exe` reads the file in the
/// system code page. On this machine - 936, measured 2026-10-07 - a Chinese literal
/// whose UTF-8 byte count is *odd* ends on a lead byte, and MSVC takes the following
/// `"` as its trail byte: `QStringLiteral("细")` becomes an unterminated constant and
/// the build dies with `C2001: newline in constant` inside `target\...\AnnotationToolbar.qml.cpp`.
///
/// It has nothing to do with that file. 复制图片 in `PinWindow.qml` is four characters
/// = twelve bytes = six complete GBK pairs, so it happens to survive; 细/中/粗 are one
/// character each and do not. A UI whose labels compile or not depending on how many
/// characters they contain is the bug being closed here, which is why the fix is the
/// flag rather than choosing safe words. It also silences the `C4819` warning Qt's own
/// headers currently raise on every build.
fn force_utf8_sources() {
    let mut flags = std::env::var("CXXFLAGS").unwrap_or_default();
    if !flags.contains("/utf-8") {
        if !flags.is_empty() {
            flags.push(' ');
        }
        flags.push_str("/utf-8");
        std::env::set_var("CXXFLAGS", &flags);
    }
}

/// Bakes the GLSL into `.qsb` and hands back a `.qrc` that puts the results next
/// to the QML that references them.
///
/// Failure is fatal, deliberately: a missing `qtshadertools` or a `qsb` that
/// errors produces a build that compiles, runs, and silently draws the mask
/// without the shader - `ShaderEffect.status` becomes `Error` at runtime, which
/// is exactly the "green CI, no shader in the package" hole this fatal-on-purpose
/// step closes.
/// The spike returned `None` and warned; the product does not.
fn bake_shaders(out_dir: &Path) -> PathBuf {
    const SOURCES: [&str; 1] = ["dim_hole.frag"];
    let qsb = find_qsb().expect("qsb.exe: install the qtshadertools module (aqt install-qt ... -m qtshadertools) or point QMAKE at a Qt that has it");

    let mut files = Vec::new();
    for shader in SOURCES {
        let source = format!("shaders/{shader}");
        println!("cargo:rerun-if-changed={source}");
        let name = format!("{shader}.qsb");
        let baked = out_dir.join(&name);
        let status = Command::new(&qsb)
            .args(["--qt6", "-o"])
            .arg(&baked)
            .arg(&source)
            .status()
            .unwrap_or_else(|e| panic!("running {}: {e}", qsb.display()));
        if !status.success() {
            panic!("qsb failed for {source} ({status}) - the mask's ShaderEffect would be missing at runtime");
        }
        files.push((name, baked));
    }

    // main.qml is loaded from qrc:/qt/qml/dev/falconshot/qml/main.qml, so a
    // relative "dim_hole.frag.qsb" only resolves if the pack sits in that same
    // prefix - the one thing about this arrangement that is not guessable.
    let mut qrc = String::from("<RCC>\n  <qresource prefix=\"/qt/qml/dev/falconshot/qml\">\n");
    // 应用自己的图标（用户 2026-11-13 放进来的那张 ✓）也进同一份 qrc：窗与托盘的图标
    // 都从 qrc 里取（`qrc:/qt/qml/dev/falconshot/qml/logo.png` ✓，斜杠不是点），于是它们不必去磁盘上
    // 找一张可能不在的图 ✓（装成 MSIX 之后 exe 旁边并没有那张 png ✓）。
    let icon = Path::new("assets/logo.png");
    println!("cargo:rerun-if-changed={}", icon.display());
    let icon_abs = icon.canonicalize().unwrap_or_else(|_| icon.to_path_buf());
    // `canonicalize()` 给的是 `\\?\C:\...` 长路径。rcc 读这种源路径没问题（2026-11-15
    // 实测嵌进二进制的数据与树路径都对），但写进 qrc 的是 `//?/C:/...` 那种怪形态，
    // 与 embed_icon（P213）同一条手 trim 掉前缀，免得下一轮读 qrc 时当它是数据。
    let icon_rel = icon_abs.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/");
    files.push(("logo.png".to_string(), PathBuf::from(icon_rel)));
    for (name, path) in files {
        let path = path.display().to_string().replace('\\', "/");
        qrc.push_str(&format!("    <file alias=\"{name}\">{path}</file>\n"));
    }
    qrc.push_str("  </qresource>\n</RCC>\n");

    let qrc_path = out_dir.join("shaders.qrc");
    std::fs::write(&qrc_path, &qrc).expect("writing shaders.qrc");
    qrc_path
}

/// `qsb` sits next to `qmake` (scripts/env.cmd exports QMAKE); fall back to
/// whatever is on PATH, which env.cmd also arranges.
fn find_qsb() -> Option<PathBuf> {
    if let Ok(qmake) = std::env::var("QMAKE") {
        let sibling = Path::new(&qmake).parent()?.join("qsb.exe");
        if sibling.exists() {
            return Some(sibling);
        }
    }
    let on_path = PathBuf::from(if cfg!(windows) { "qsb.exe" } else { "qsb" });
    Some(on_path)
}

/// P213: 把 `assets/app.ico`（`packaging/make_ico.ps1` 从用户的 logo 打出来的那枚）
/// 经 rc.exe 编成资源文件，交给链接器进 exe —— 资源号 #1。
///
/// 取这一枚的三处：托盘那一张脸（platform-windows 的 `pump_thread` 用
/// `GetModuleHandleW(None)` 从本进程模块取）、资源管理器里的文件图标、以及窗口没
/// 自设图标时任务栏给的那张默认脸。失败是致命的，与 qsb 同一条理：编译出来一枚
/// 没有脸的 exe，它照样能跑，缺的那一块就没人再看见了。
///
/// 链接用 `cargo:rustc-link-arg-bins` 把 .res 当链接器输入递过去 —— link.exe 收
/// .res ✓。这是 build.rs 自己能闭环的一条路，不为此引入新的构建依赖。
fn embed_icon(out_dir: &Path) {
    let icon = Path::new("assets/app.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let abs = icon
        .canonicalize()
        .unwrap_or_else(|_| panic!("assets/app.ico 缺失：先跑 packaging/make_ico.ps1"));
    let abs = abs.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/");
    let rc_source = out_dir.join("app_icon.rc");
    std::fs::write(&rc_source, format!("1 ICON \"{abs}\"\n")).expect("writing app_icon.rc");
    let res = out_dir.join("app_icon.res");
    let rc_exe = find_rc();
    let status = Command::new(&rc_exe)
        .args(["/nologo", "/fo"])
        .arg(&res)
        .arg(&rc_source)
        .status()
        .unwrap_or_else(|e| panic!("running {}: {e}", rc_exe.display()));
    if !status.success() {
        panic!("rc.exe failed ({status}) - the exe icon resource would be missing");
    }
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

/// `rc.exe` 与 `makeappx.exe` 同住 Windows Kits 的 bin 一夹；逐版本从新到旧找，
/// 找不着就交 PATH。
fn find_rc() -> PathBuf {
    let kits = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    if let Ok(entries) = std::fs::read_dir(kits) {
        let mut versions: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        versions.sort();
        for version in versions.iter().rev() {
            let candidate = version.join("x64").join("rc.exe");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from("rc.exe")
}

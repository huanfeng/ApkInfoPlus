//! Windows 目标：把 `assets/windows/app.rc`（exe 图标 + 版本信息）编译成 .res 并链接进可执行文件。
//!
//! 不引入 winres 之类的依赖：只要找到一个资源编译器就行——MSVC 的 `rc.exe`、LLVM 的
//! `llvm-rc`（cargo-xwin 交叉编译环境自带）、或 MinGW 的 `windres`。一个都没有时只给出
//! 警告，可执行文件照常生成，只是没有图标。

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/windows/app.rc");
    println!("cargo:rerun-if-changed=assets/windows/app.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let rc = manifest.join("assets/windows/app.rc");
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("app.res");
    let gnu = env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu");

    let attempts: Vec<(&str, Vec<String>)> = if gnu {
        vec![
            ("x86_64-w64-mingw32-windres", windres_args(&rc, &out)),
            ("windres", windres_args(&rc, &out)),
        ]
    } else {
        vec![
            ("rc.exe", rc_args(&rc, &out)),
            ("llvm-rc", rc_args(&rc, &out)),
            ("llvm-rc-19", rc_args(&rc, &out)),
            ("llvm-rc-18", rc_args(&rc, &out)),
        ]
    };
    for (tool, args) in attempts {
        let ok = Command::new(tool)
            .args(&args)
            .current_dir(rc.parent().unwrap())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok && out.exists() {
            println!("cargo:rustc-link-arg-bins={}", out.display());
            return;
        }
    }
    println!("cargo:warning=未找到资源编译器（rc.exe / llvm-rc / windres），可执行文件将没有图标");
}

fn rc_args(rc: &std::path::Path, out: &std::path::Path) -> Vec<String> {
    vec![
        "/nologo".into(),
        "/FO".into(),
        out.display().to_string(),
        rc.display().to_string(),
    ]
}

fn windres_args(rc: &std::path::Path, out: &std::path::Path) -> Vec<String> {
    vec![
        rc.display().to_string(),
        "-O".into(),
        "coff".into(),
        "-o".into(),
        out.display().to_string(),
    ]
}

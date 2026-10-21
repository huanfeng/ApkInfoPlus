//! 文件类型关联：把 .apk / .xapk / .apkm / .apks 的双击打开指向本程序。
//!
//! - Windows：按当前用户写 `HKCU\Software\Classes`（不需要管理员），注册 ProgID、
//!   `OpenWithProgids` 与 `Applications\apkinfoplus.exe`，并把扩展名默认值指过来。
//!   Windows 10 起若用户在「打开方式」里选过别的程序（UserChoice），系统仍以那份为准，
//!   这时只能让用户去「默认应用」里选，所以另给一个打开系统设置的入口。
//! - Linux：写 `~/.local/share` 下的 .desktop 与 MIME 定义，再用 `xdg-mime` 设为默认。
//! - macOS：关联由 .app 包的 Info.plist 声明，程序里不做。

use std::path::PathBuf;
use std::process::Command;

/// 关联的扩展名（不含点）。`.zip` 太宽泛，不抢。
pub const EXTENSIONS: [&str; 4] = ["apk", "xapk", "apkm", "apks"];

pub fn is_supported() -> bool {
    cfg!(any(target_os = "windows", target_os = "linux"))
}

fn current_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| e.to_string())
}

/// 当前是否已关联到本程序（所有扩展名都指向当前 exe 才算）。
pub fn is_registered() -> bool {
    imp::is_registered()
}

pub fn register() -> Result<(), String> {
    imp::register()
}

pub fn unregister() -> Result<(), String> {
    imp::unregister()
}

/// 打开系统的「默认应用」设置页。
pub fn open_system_settings() -> Result<(), String> {
    imp::open_system_settings()
}

fn run(cmd: &mut Command) -> Result<String, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：reg.exe 不要闪控制台窗口。
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            format!("exit {}", out.status)
        } else {
            err
        })
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;

    const CLASSES: &str = r"HKCU\Software\Classes";
    const EXE_NAME: &str = "apkinfoplus.exe";

    fn prog_id(ext: &str) -> String {
        format!("ApkInfoPlus.{}", ext)
    }

    fn reg_set(key: &str, name: Option<&str>, value: &str) -> Result<(), String> {
        let mut c = Command::new("reg");
        c.args(["add", key]);
        match name {
            Some(n) => c.args(["/v", n]),
            None => c.arg("/ve"),
        };
        c.args(["/t", "REG_SZ", "/d", value, "/f"]);
        run(&mut c).map(|_| ())
    }

    /// 读键的默认值；键或值不存在返回 None。直接调 API：reg.exe 的输出走 OEM 代码页，
    /// 路径里有中文时按 UTF-8 解出来是乱码，没法与当前 exe 路径比对。
    fn reg_default(key: &str) -> Option<String> {
        #[link(name = "advapi32")]
        extern "system" {
            fn RegGetValueW(
                hkey: isize,
                sub_key: *const u16,
                value: *const u16,
                flags: u32,
                ty: *mut u32,
                data: *mut std::ffi::c_void,
                len: *mut u32,
            ) -> i32;
        }
        const HKEY_CURRENT_USER: isize = 0x8000_0001_u32 as i32 as isize;
        const RRF_RT_REG_SZ: u32 = 0x0000_0002;
        let sub = key.strip_prefix(r"HKCU\")?;
        let wide: Vec<u16> = sub.encode_utf16().chain(Some(0)).collect();
        let mut buf = vec![0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                wide.as_ptr(),
                std::ptr::null(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return None;
        }
        let n = (len as usize / 2).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..n]);
        Some(s.trim_end_matches('\0').to_string())
    }

    fn reg_delete_key(key: &str) {
        let _ = run(Command::new("reg").args(["delete", key, "/f"]));
    }

    fn open_command(exe: &std::path::Path) -> String {
        format!("\"{}\" \"%1\"", exe.display())
    }

    pub fn is_registered() -> bool {
        let Ok(exe) = current_exe() else {
            return false;
        };
        let want = open_command(&exe);
        EXTENSIONS.iter().all(|ext| {
            let pid = prog_id(ext);
            reg_default(&format!(r"{}\{}\shell\open\command", CLASSES, pid))
                .is_some_and(|v| v.eq_ignore_ascii_case(&want))
                && reg_default(&format!(r"{}\.{}", CLASSES, ext)).as_deref() == Some(pid.as_str())
        })
    }

    pub fn register() -> Result<(), String> {
        let exe = current_exe()?;
        let cmd = open_command(&exe);
        let icon = format!("\"{}\",0", exe.display());
        for ext in EXTENSIONS {
            let pid = prog_id(ext);
            let pkey = format!(r"{}\{}", CLASSES, pid);
            reg_set(
                &pkey,
                None,
                &format!("{} ({})", crate::state::APP_TITLE, ext.to_uppercase()),
            )?;
            reg_set(&format!(r"{}\DefaultIcon", pkey), None, &icon)?;
            reg_set(&format!(r"{}\shell\open\command", pkey), None, &cmd)?;
            let ekey = format!(r"{}\.{}", CLASSES, ext);
            reg_set(&ekey, None, &pid)?;
            run(Command::new("reg").args([
                "add",
                &format!(r"{}\OpenWithProgids", ekey),
                "/v",
                &pid,
                "/t",
                "REG_NONE",
                "/f",
            ]))?;
        }
        // 「打开方式」列表里显示友好名称、并声明支持的类型。
        let akey = format!(r"{}\Applications\{}", CLASSES, EXE_NAME);
        reg_set(&akey, Some("FriendlyAppName"), crate::state::APP_TITLE)?;
        reg_set(&format!(r"{}\shell\open\command", akey), None, &cmd)?;
        for ext in EXTENSIONS {
            reg_set(
                &format!(r"{}\SupportedTypes", akey),
                Some(&format!(".{}", ext)),
                "",
            )?;
        }
        notify_changed();
        Ok(())
    }

    pub fn unregister() -> Result<(), String> {
        for ext in EXTENSIONS {
            let pid = prog_id(ext);
            let ekey = format!(r"{}\.{}", CLASSES, ext);
            if reg_default(&ekey).as_deref() == Some(pid.as_str()) {
                let _ = run(Command::new("reg").args(["delete", &ekey, "/ve", "/f"]));
            }
            let _ = run(Command::new("reg").args([
                "delete",
                &format!(r"{}\OpenWithProgids", ekey),
                "/v",
                &pid,
                "/f",
            ]));
            reg_delete_key(&format!(r"{}\{}", CLASSES, pid));
        }
        reg_delete_key(&format!(r"{}\Applications\{}", CLASSES, EXE_NAME));
        notify_changed();
        Ok(())
    }

    /// 通知资源管理器关联已变，文件图标立即刷新。
    fn notify_changed() {
        #[link(name = "shell32")]
        extern "system" {
            fn SHChangeNotify(
                event: i32,
                flags: u32,
                item1: *const std::ffi::c_void,
                item2: *const std::ffi::c_void,
            );
        }
        const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
        const SHCNF_IDLIST: u32 = 0;
        unsafe {
            SHChangeNotify(
                SHCNE_ASSOCCHANGED,
                SHCNF_IDLIST,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
    }

    pub fn open_system_settings() -> Result<(), String> {
        Command::new("explorer.exe")
            .arg("ms-settings:defaultapps")
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    const DESKTOP: &str = "apkinfoplus.desktop";
    const MIME_XML: &str = "apkinfoplus.xml";
    /// APK 有标准 MIME；其余三种没有，用自定义类型声明（按扩展名匹配）。
    const MIMES: [(&str, &str); 4] = [
        ("apk", "application/vnd.android.package-archive"),
        ("xapk", "application/x-xapk"),
        ("apkm", "application/x-apkm"),
        ("apks", "application/x-apks"),
    ];

    fn data_home() -> Option<PathBuf> {
        dirs::data_dir()
    }

    fn desktop_path() -> Option<PathBuf> {
        data_home().map(|d| d.join("applications").join(DESKTOP))
    }

    pub fn is_registered() -> bool {
        let (Some(p), Ok(exe)) = (desktop_path(), current_exe()) else {
            return false;
        };
        std::fs::read_to_string(p)
            .map(|s| s.contains(&format!("Exec=\"{}\"", exe.display())))
            .unwrap_or(false)
    }

    pub fn register() -> Result<(), String> {
        let exe = current_exe()?;
        let data = data_home().ok_or("no data dir")?;
        let apps = data.join("applications");
        let mime_pkgs = data.join("mime").join("packages");
        std::fs::create_dir_all(&apps).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&mime_pkgs).map_err(|e| e.to_string())?;

        let mut xml = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\">\n",
        );
        for (ext, mime) in MIMES.iter().skip(1) {
            xml.push_str(&format!(
                "  <mime-type type=\"{mime}\">\n    <comment>{} package</comment>\n    \
                 <sub-class-of type=\"application/zip\"/>\n    <glob pattern=\"*.{ext}\"/>\n  </mime-type>\n",
                ext.to_uppercase()
            ));
        }
        xml.push_str("</mime-info>\n");
        std::fs::write(mime_pkgs.join(MIME_XML), xml).map_err(|e| e.to_string())?;

        let mimes: Vec<&str> = MIMES.iter().map(|(_, m)| *m).collect();
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName={}\nExec=\"{}\" %F\nTerminal=false\n\
             Categories=Development;Utility;\nMimeType={};\n",
            crate::state::APP_TITLE,
            exe.display(),
            mimes.join(";")
        );
        std::fs::write(apps.join(DESKTOP), desktop).map_err(|e| e.to_string())?;

        let _ = run(Command::new("update-mime-database").arg(data.join("mime")));
        let _ = run(Command::new("update-desktop-database").arg(&apps));
        let mut c = Command::new("xdg-mime");
        c.args(["default", DESKTOP]).args(&mimes);
        run(&mut c).map(|_| ())
    }

    pub fn unregister() -> Result<(), String> {
        let data = data_home().ok_or("no data dir")?;
        let _ = std::fs::remove_file(data.join("applications").join(DESKTOP));
        let _ = std::fs::remove_file(data.join("mime").join("packages").join(MIME_XML));
        let _ = run(Command::new("update-mime-database").arg(data.join("mime")));
        let _ = run(Command::new("update-desktop-database").arg(data.join("applications")));
        Ok(())
    }

    pub fn open_system_settings() -> Result<(), String> {
        Err("unsupported".into())
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    pub fn is_registered() -> bool {
        false
    }
    pub fn register() -> Result<(), String> {
        Err("unsupported".into())
    }
    pub fn unregister() -> Result<(), String> {
        Err("unsupported".into())
    }
    pub fn open_system_settings() -> Result<(), String> {
        Err("unsupported".into())
    }
}

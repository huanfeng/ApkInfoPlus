//! adb 封装：设备列表、`install` / `install-multiple` / `push` / `shell`，支持取消。

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbDevice {
    pub id: String,
    /// `device` / `offline` / `unauthorized` / …
    pub status: String,
    pub product: Option<String>,
    pub model: Option<String>,
    pub device: Option<String>,
}

impl AdbDevice {
    pub fn is_offline(&self) -> bool {
        self.status == "offline"
    }
    pub fn is_unauthorized(&self) -> bool {
        self.status == "unauthorized"
    }
    /// `ip:port` 形式即无线连接。
    pub fn is_wireless(&self) -> bool {
        self.id.contains(':')
    }
    pub fn display_name(&self) -> String {
        if self.is_wireless() {
            return format!("IP: {}", self.id);
        }
        let mut parts = Vec::new();
        if let Some(m) = &self.model {
            parts.push(m.replace('_', " "));
        }
        if let Some(d) = &self.device {
            parts.push(d.clone());
        }
        if parts.is_empty() {
            self.id.clone()
        } else {
            parts.join(" ")
        }
    }
}

/// 解析 `adb devices -l` 的输出。
pub fn parse_devices(output: &str) -> Vec<AdbDevice> {
    let mut out = Vec::new();
    for line in output
        .lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
    {
        let line = line.trim();
        if line.is_empty() || line.starts_with('*') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(id), Some(status)) = (parts.next(), parts.next()) else {
            continue;
        };
        let mut dev = AdbDevice {
            id: id.to_string(),
            status: status.to_string(),
            product: None,
            model: None,
            device: None,
        };
        for extra in parts {
            if let Some(v) = extra.strip_prefix("product:") {
                dev.product = Some(v.to_string());
            } else if let Some(v) = extra.strip_prefix("model:") {
                dev.model = Some(v.to_string());
            } else if let Some(v) = extra.strip_prefix("device:") {
                dev.device = Some(v.to_string());
            }
        }
        out.push(dev);
    }
    out
}

fn command(adb: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(adb);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：不要为 adb 弹出控制台窗口。
        c.creation_flags(0x0800_0000);
    }
    c
}

pub fn list_devices(adb: &Path) -> Result<Vec<AdbDevice>, String> {
    let out = command(adb)
        .args(["devices", "-l"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{}: {}", adb.display(), e))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            format!("adb exit {}", out.status)
        } else {
            err
        });
    }
    Ok(parse_devices(&String::from_utf8_lossy(&out.stdout)))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InstallOptions {
    pub allow_test: bool,
    pub allow_downgrade: bool,
    pub replace: bool,
    pub grant_permissions: bool,
}

impl InstallOptions {
    /// 选项顺序固定：-r -d -t -g。
    pub fn flags(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.replace {
            v.push("-r");
        }
        if self.allow_downgrade {
            v.push("-d");
        }
        if self.allow_test {
            v.push("-t");
        }
        if self.grant_permissions {
            v.push("-g");
        }
        v
    }
}

/// 子进程执行结果。
#[derive(Debug, Clone, Default)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub cancelled: bool,
}

impl Output {
    /// adb install 的失败原因常在 stdout 的 `Failure [...]` 里，而退出码仍可能是 0。
    pub fn failure_reason(&self) -> Option<String> {
        for text in [&self.stdout, &self.stderr] {
            if let Some(idx) = text.find("Failure") {
                let line = text[idx..].lines().next().unwrap_or("").trim();
                return Some(line.to_string());
            }
            if let Some(idx) = text.find("Exception") {
                let line = text[idx..].lines().next().unwrap_or("").trim();
                return Some(line.to_string());
            }
        }
        if !self.success {
            let s = self.stderr.trim();
            if !s.is_empty() {
                return Some(s.lines().last().unwrap_or(s).to_string());
            }
            let o = self.stdout.trim();
            if !o.is_empty() {
                return Some(o.lines().last().unwrap_or(o).to_string());
            }
            return Some("exit code != 0".into());
        }
        None
    }

    pub fn ok(&self) -> bool {
        self.success && !self.cancelled && self.failure_reason().is_none()
    }
}

/// 跑一条 adb 命令，期间每 100ms 检查取消标志，取消即 kill。
pub fn run(adb: &Path, args: &[String], cancel: &Arc<AtomicBool>) -> Result<Output, String> {
    let mut child = command(adb)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {}", adb.display(), e))?;

    // stdout / stderr 分别在线程里读，避免管道写满死锁。
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_thread = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut s);
        }
        s
    });
    let err_thread = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut s);
        }
        s
    });

    let mut cancelled = false;
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            cancelled = true;
        }
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let stdout = String::from_utf8_lossy(&out_thread.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&err_thread.join().unwrap_or_default()).into_owned();
    Ok(Output {
        success: status.success(),
        stdout,
        stderr,
        cancelled,
    })
}

pub fn install_args(device: &str, opts: &InstallOptions, apk: &Path) -> Vec<String> {
    let mut v = vec!["-s".to_string(), device.to_string(), "install".to_string()];
    v.extend(opts.flags().iter().map(|s| s.to_string()));
    v.push(apk.display().to_string());
    v
}

pub fn install_multiple_args(
    device: &str,
    opts: &InstallOptions,
    apks: &[std::path::PathBuf],
) -> Vec<String> {
    let mut v = vec![
        "-s".to_string(),
        device.to_string(),
        "install-multiple".to_string(),
    ];
    v.extend(opts.flags().iter().map(|s| s.to_string()));
    v.extend(apks.iter().map(|p| p.display().to_string()));
    v
}

pub fn shell_mkdir_args(device: &str, dir: &str) -> Vec<String> {
    vec![
        "-s".into(),
        device.into(),
        "shell".into(),
        "mkdir".into(),
        "-p".into(),
        dir.to_string(),
    ]
}

pub fn push_args(device: &str, local: &Path, remote: &str) -> Vec<String> {
    vec![
        "-s".into(),
        device.into(),
        "push".into(),
        local.display().to_string(),
        remote.to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_devices_output() {
        let text = "List of devices attached\n\
                    emulator-5554          device product:sdk_gphone64_x86_64 model:sdk_gphone64_x86_64 device:emu64xa transport_id:1\n\
                    192.168.1.5:5555       offline\n\
                    ABCDEF                 unauthorized\n\n";
        let d = parse_devices(text);
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].id, "emulator-5554");
        assert_eq!(d[0].model.as_deref(), Some("sdk_gphone64_x86_64"));
        assert_eq!(d[0].display_name(), "sdk gphone64 x86 64 emu64xa");
        assert!(d[1].is_offline());
        assert!(d[1].is_wireless());
        assert_eq!(d[1].display_name(), "IP: 192.168.1.5:5555");
        assert!(d[2].is_unauthorized());
    }

    #[test]
    fn args_order() {
        let o = InstallOptions {
            allow_test: true,
            allow_downgrade: true,
            replace: true,
            grant_permissions: false,
        };
        let a = install_args("dev", &o, Path::new("/x/a.apk"));
        assert_eq!(
            a,
            vec!["-s", "dev", "install", "-r", "-d", "-t", "/x/a.apk"]
        );
    }

    #[test]
    fn failure_detection() {
        let o = Output {
            success: true,
            stdout: "Performing Streamed Install\nFailure [INSTALL_FAILED_UPDATE_INCOMPATIBLE]\n"
                .into(),
            stderr: String::new(),
            cancelled: false,
        };
        assert!(!o.ok());
        assert_eq!(
            o.failure_reason().unwrap(),
            "Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE]"
        );
        let ok = Output {
            success: true,
            stdout: "Success\n".into(),
            ..Default::default()
        };
        assert!(ok.ok());
    }
}

//! 日志：stderr 总是输出 warn 以上；开启调试时额外写 `debug.log`（2MB 滚动一份备份）。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use log::{LevelFilter, Log, Metadata, Record};

static DEBUG: AtomicBool = AtomicBool::new(false);
static FILE: Mutex<Option<File>> = Mutex::new(None);

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

struct Logger;

impl Log for Logger {
    fn enabled(&self, m: &Metadata) -> bool {
        m.level() <= LevelFilter::Debug
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // 只记录本工作区的两个 crate，屏蔽依赖库的噪音。
        let target = record.target();
        if !(target.starts_with("apkinfo") || target.starts_with("windui")) {
            return;
        }
        let line = format!(
            "{} [{}] {}: {}",
            timestamp(),
            record.level(),
            target,
            record.args()
        );
        if record.level() <= log::Level::Warn || DEBUG.load(Ordering::Relaxed) {
            eprintln!("{}", line);
        }
        if DEBUG.load(Ordering::Relaxed) {
            if let Ok(mut guard) = FILE.lock() {
                if guard.is_none() {
                    *guard = open_log_file();
                }
                if let Some(f) = guard.as_mut() {
                    let _ = writeln!(f, "{}", line);
                }
            }
        }
    }

    fn flush(&self) {}
}

fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let ms = now.subsec_millis();
    // 无时区库：按 UTC 拆年月日时分秒，够用于排查顺序。
    let days = secs / 86400;
    let (y, m, d) = civil_from_days(days as i64);
    let rem = secs % 86400;
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}Z",
        y,
        m,
        d,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
        ms
    )
}

/// Howard Hinnant 的 days → civil 算法。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn open_log_file() -> Option<File> {
    let path = crate::config::log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() >= MAX_LOG_BYTES {
            let backup = path.with_file_name("debug.1.log");
            let _ = std::fs::remove_file(&backup);
            let _ = std::fs::rename(&path, &backup);
        }
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()
}

pub fn init(debug: bool) {
    DEBUG.store(debug, Ordering::Relaxed);
    let _ = log::set_logger(&Logger);
    log::set_max_level(LevelFilter::Debug);
}

pub fn set_debug(on: bool) {
    DEBUG.store(on, Ordering::Relaxed);
    if !on {
        if let Ok(mut g) = FILE.lock() {
            *g = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }
}

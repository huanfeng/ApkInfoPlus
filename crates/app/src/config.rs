//! 应用配置：TOML 文件，位于平台配置目录下的 `apkinfoplus/config.toml`。
//!
//! 字段都给默认值，缺失或类型不对的字段回退默认而不是整份失败——用户手改配置时
//! 一处笔误不应把全部设置清零。

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const APP_DIR_NAME: &str = "apkinfoplus";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Light,
    Dark,
    #[default]
    System,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 3] = [ThemeMode::Light, ThemeMode::Dark, ThemeMode::System];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolSource {
    /// 先看下载目录，再看 PATH，最后自定义。
    #[default]
    Builtin,
    /// 先看 PATH。
    System,
    /// 先用自定义路径。
    Custom,
}

impl ToolSource {
    pub const ALL: [ToolSource; 3] = [ToolSource::Builtin, ToolSource::System, ToolSource::Custom];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `auto` 跟随系统，否则是 BCP-47（`zh-CN` / `en`）。
    pub language: String,
    pub theme_mode: ThemeMode,
    /// `0xRRGGBB`；`None` 用 windui 默认强调色。
    pub accent_color: Option<u32>,
    /// 字段标题列宽（逻辑像素）。
    pub title_width: i32,
    pub enable_signature: bool,
    pub enable_hash: bool,
    pub enable_debug: bool,
    pub adb_source: ToolSource,
    pub adb_path: String,
    /// 空表示 `<data_dir>/bin`。
    pub download_dir: String,
    pub window_width: i32,
    pub window_height: i32,
    /// 左侧文件列表占窗口宽度的比例（分隔条可拖）。
    pub sidebar_ratio: f32,
    /// 上次打开的目录，文件对话框从这里开始。
    pub last_dir: String,
    /// 安装对话框的高级选项，记住上次勾选。
    pub install_allow_test: bool,
    pub install_allow_downgrade: bool,
    pub install_replace: bool,
    pub install_grant: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            theme_mode: ThemeMode::System,
            accent_color: None,
            title_width: 110,
            enable_signature: true,
            enable_hash: true,
            enable_debug: false,
            adb_source: ToolSource::Builtin,
            adb_path: String::new(),
            download_dir: String::new(),
            window_width: 1180,
            window_height: 760,
            sidebar_ratio: 0.22,
            last_dir: String::new(),
            install_allow_test: false,
            install_allow_downgrade: false,
            install_replace: true,
            install_grant: false,
        }
    }
}

/// 配置目录（`~/.config/apkinfoplus` 等）。
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

/// 数据目录：日志、下载的 platform-tools。
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn log_path() -> PathBuf {
    data_dir().join("debug.log")
}

impl Config {
    pub fn load() -> Config {
        let path = config_path();
        match fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("配置文件解析失败，使用默认值: {} ({})", e, path.display());
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = fs::write(&path, text) {
                    log::warn!("写配置失败: {} ({})", e, path.display());
                }
            }
            Err(e) => log::warn!("序列化配置失败: {}", e),
        }
    }

    /// 实际的下载目录。
    pub fn resolved_download_dir(&self) -> PathBuf {
        if self.download_dir.trim().is_empty() {
            data_dir().join("bin")
        } else {
            let p = PathBuf::from(&self.download_dir);
            if p.is_absolute() {
                p
            } else {
                data_dir().join(p)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let c = Config {
            language: "en".into(),
            theme_mode: ThemeMode::Dark,
            accent_color: Some(0x2E9E5B),
            ..Config::default()
        };
        let text = toml::to_string(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn missing_fields_fall_back() {
        let back: Config = toml::from_str("language = \"zh-CN\"\n").unwrap();
        assert_eq!(back.language, "zh-CN");
        assert_eq!(back.title_width, Config::default().title_width);
        assert!(back.enable_hash);
    }
}

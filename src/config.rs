use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

use crate::file_list::SortOrder;
use crate::render::d2d_renderer::AlphaBackground;
use crate::render::layout::DisplayMode;

/// アプリケーション設定（gv3.toml）
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub display: DisplayConfig,
    pub prefetch: PrefetchConfig,
    pub list: ListConfig,
    pub window: WindowConfig,
    pub susie: SusieConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// 表示モード: "shrink", "fit", "enlarge", "original", "fixed"
    pub auto_scale: String,
    /// 固定倍率（auto_scale = "fixed" のとき使用）
    pub fixed_scale: f32,
    /// 余白量（ピクセル）
    pub margin: f32,
    /// α背景: "white", "black", "checker"
    pub alpha_background: String,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct PrefetchConfig {
    pub cache_base_width: u32,
    pub cache_base_height: u32,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct ListConfig {
    /// デフォルトソート: "name", "name_nocase", "size", "date", "natural"
    pub default_sort: String,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    pub remember_position: bool,
    pub remember_size: bool,
    pub always_on_top: bool,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct SusieConfig {
    pub plugin_dir: String,
    /// 画像プラグイン優先度（上が高優先）
    pub image_plugins: Vec<String>,
    /// アーカイブプラグイン優先度
    pub archive_plugins: Vec<String>,
}

// --- Default実装 ---

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            auto_scale: "shrink".to_string(),
            fixed_scale: 1.0,
            margin: 20.0,
            alpha_background: "checker".to_string(),
        }
    }
}

impl Default for PrefetchConfig {
    fn default() -> Self {
        Self {
            cache_base_width: 1024,
            cache_base_height: 1536,
        }
    }
}

impl Default for ListConfig {
    fn default() -> Self {
        Self {
            default_sort: "name".to_string(),
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            remember_position: true,
            remember_size: true,
            always_on_top: false,
        }
    }
}

impl Default for SusieConfig {
    fn default() -> Self {
        Self {
            plugin_dir: "spi".to_string(),
            image_plugins: Vec::new(),
            archive_plugins: Vec::new(),
        }
    }
}

// --- 変換ヘルパー ---

impl DisplayConfig {
    /// 文字列からDisplayModeに変換
    pub fn to_display_mode(&self) -> DisplayMode {
        match self.auto_scale.as_str() {
            "shrink" => DisplayMode::AutoShrink,
            "fit" => DisplayMode::AutoFit,
            "enlarge" => DisplayMode::AutoEnlarge,
            "original" => DisplayMode::Original,
            "fixed" => DisplayMode::Fixed(self.fixed_scale),
            _ => DisplayMode::AutoShrink,
        }
    }

    /// 文字列からAlphaBackgroundに変換
    pub fn to_alpha_background(&self) -> AlphaBackground {
        match self.alpha_background.as_str() {
            "white" => AlphaBackground::White,
            "black" => AlphaBackground::Black,
            "checker" => AlphaBackground::Checker,
            _ => AlphaBackground::Checker,
        }
    }
}

impl ListConfig {
    /// 文字列からSortOrderに変換
    pub fn to_sort_order(&self) -> SortOrder {
        match self.default_sort.as_str() {
            "name" => SortOrder::Name,
            "name_nocase" => SortOrder::NameNoCase,
            "size" => SortOrder::Size,
            "date" => SortOrder::Date,
            "natural" => SortOrder::Natural,
            _ => SortOrder::Name,
        }
    }
}

impl PrefetchConfig {
    /// キャッシュ基準サイズ（バイト）
    pub fn base_image_size(&self) -> usize {
        self.cache_base_width as usize * self.cache_base_height as usize * 4
    }
}

impl Config {
    /// exeディレクトリの `gv3.toml` を読み込む。
    /// ファイルなし / パース失敗はデフォルトにフォールバック。
    pub fn load() -> Self {
        let Some(config_path) = Self::config_path() else {
            return Config::default();
        };
        Self::load_from(&config_path).unwrap_or_default()
    }

    /// 指定パスからConfigを読み込む
    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// 設定ファイルのパスを返す（exeと同じディレクトリの gv3.toml）
    fn config_path() -> Option<std::path::PathBuf> {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("gv3.toml")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = Config::default();
        assert_eq!(config.display.auto_scale, "shrink");
        assert_eq!(config.display.fixed_scale, 1.0);
        assert_eq!(config.display.margin, 20.0);
        assert_eq!(config.display.alpha_background, "checker");
        assert_eq!(config.prefetch.cache_base_width, 1024);
        assert_eq!(config.prefetch.cache_base_height, 1536);
        assert_eq!(config.list.default_sort, "name");
        assert!(config.window.remember_position);
        assert!(config.window.remember_size);
        assert!(!config.window.always_on_top);
        assert_eq!(config.susie.plugin_dir, "spi");
    }

    #[test]
    fn parse_full_toml() {
        let toml_str = r#"
[display]
auto_scale = "fit"
fixed_scale = 2.0
margin = 10.0
alpha_background = "black"

[prefetch]
cache_base_width = 800
cache_base_height = 600

[list]
default_sort = "natural"

[window]
remember_position = false
remember_size = false
always_on_top = true

[susie]
plugin_dir = "plugins"
image_plugins = ["ifwebp.sph"]
archive_plugins = ["axlha.sph"]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.display.auto_scale, "fit");
        assert_eq!(config.display.fixed_scale, 2.0);
        assert_eq!(config.display.margin, 10.0);
        assert_eq!(config.display.alpha_background, "black");
        assert_eq!(config.prefetch.cache_base_width, 800);
        assert_eq!(config.list.default_sort, "natural");
        assert!(!config.window.remember_position);
        assert!(config.window.always_on_top);
        assert_eq!(config.susie.plugin_dir, "plugins");
        assert_eq!(config.susie.image_plugins, vec!["ifwebp.sph"]);
    }

    #[test]
    fn parse_partial_toml_fills_defaults() {
        let toml_str = r#"
[display]
auto_scale = "original"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.display.auto_scale, "original");
        // 未指定フィールドはデフォルト
        assert_eq!(config.display.margin, 20.0);
        assert_eq!(config.list.default_sort, "name");
    }

    #[test]
    fn display_mode_conversion() {
        let d = DisplayConfig {
            auto_scale: "shrink".into(),
            ..Default::default()
        };
        assert_eq!(d.to_display_mode(), DisplayMode::AutoShrink);

        let d = DisplayConfig {
            auto_scale: "fit".into(),
            ..Default::default()
        };
        assert_eq!(d.to_display_mode(), DisplayMode::AutoFit);

        let d = DisplayConfig {
            auto_scale: "fixed".into(),
            fixed_scale: 2.5,
            ..Default::default()
        };
        assert_eq!(d.to_display_mode(), DisplayMode::Fixed(2.5));

        // 不明な値はAutoShrinkフォールバック
        let d = DisplayConfig {
            auto_scale: "unknown".into(),
            ..Default::default()
        };
        assert_eq!(d.to_display_mode(), DisplayMode::AutoShrink);
    }

    #[test]
    fn alpha_background_conversion() {
        let d = DisplayConfig {
            alpha_background: "white".into(),
            ..Default::default()
        };
        assert_eq!(d.to_alpha_background(), AlphaBackground::White);

        let d = DisplayConfig {
            alpha_background: "black".into(),
            ..Default::default()
        };
        assert_eq!(d.to_alpha_background(), AlphaBackground::Black);

        let d = DisplayConfig {
            alpha_background: "checker".into(),
            ..Default::default()
        };
        assert_eq!(d.to_alpha_background(), AlphaBackground::Checker);
    }

    #[test]
    fn sort_order_conversion() {
        let l = ListConfig {
            default_sort: "natural".into(),
        };
        assert_eq!(l.to_sort_order(), SortOrder::Natural);

        let l = ListConfig {
            default_sort: "size".into(),
        };
        assert_eq!(l.to_sort_order(), SortOrder::Size);
    }

    #[test]
    fn prefetch_base_image_size() {
        let p = PrefetchConfig::default();
        assert_eq!(p.base_image_size(), 1024 * 1536 * 4);
    }
}

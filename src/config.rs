use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

use crate::file_list::SortOrder;
use crate::render::d2d_renderer::AlphaBackground;
use crate::render::layout::DisplayMode;

/// 設定ファイル上の表示モード（DisplayModeへの中間表現）
/// Fixed(f32)はデータ付きバリアントのため、serde直接対応は不可。
/// config側でfixed_scaleと組み合わせてDisplayModeに変換する。
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayModeConfig {
    Shrink,
    #[default]
    Fit,
    Enlarge,
    Original,
    Fixed,
}

/// アプリケーション設定（ぐらびゅ.toml）
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub display: DisplayConfig,
    pub prefetch: PrefetchConfig,
    pub list: ListConfig,
    pub window: WindowConfig,
    pub susie: SusieConfig,
    pub slideshow: SlideshowConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// 表示モード
    #[serde(deserialize_with = "deserialize_display_mode_config")]
    pub auto_scale: DisplayModeConfig,
    /// 固定倍率（auto_scale = Fixed のとき使用）
    pub fixed_scale: f32,
    /// 余白量（ピクセル）
    pub margin: f32,
    /// α背景
    #[serde(deserialize_with = "deserialize_alpha_background")]
    pub alpha_background: AlphaBackground,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct PrefetchConfig {
    pub cache_base_width: u32,
    pub cache_base_height: u32,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListConfig {
    /// デフォルトソート
    #[serde(deserialize_with = "deserialize_sort_order")]
    pub default_sort: SortOrder,
}

#[derive(Debug, Default, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct SlideshowConfig {
    /// スライドショー間隔（ミリ秒）
    pub interval_ms: u32,
    /// 最後の画像の後に最初に戻る
    pub repeat: bool,
}

// --- カスタムデシリアライザ（フィールド単位フォールバック + stderr警告） ---

fn deserialize_display_mode_config<'de, D>(deserializer: D) -> Result<DisplayModeConfig, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    match s.as_str() {
        "shrink" => Ok(DisplayModeConfig::Shrink),
        "fit" => Ok(DisplayModeConfig::Fit),
        "enlarge" => Ok(DisplayModeConfig::Enlarge),
        "original" => Ok(DisplayModeConfig::Original),
        "fixed" => Ok(DisplayModeConfig::Fixed),
        unknown => {
            eprintln!(
                "警告: auto_scale の値 '{unknown}' は無効です。デフォルト(fit)を使用します。"
            );
            Ok(DisplayModeConfig::Fit)
        }
    }
}

fn deserialize_alpha_background<'de, D>(deserializer: D) -> Result<AlphaBackground, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    match s.as_str() {
        "white" => Ok(AlphaBackground::White),
        "black" => Ok(AlphaBackground::Black),
        "checker" => Ok(AlphaBackground::Checker),
        unknown => {
            eprintln!(
                "警告: alpha_background の値 '{unknown}' は無効です。デフォルト(checker)を使用します。"
            );
            Ok(AlphaBackground::Checker)
        }
    }
}

fn deserialize_sort_order<'de, D>(deserializer: D) -> Result<SortOrder, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    match s.as_str() {
        "name" => Ok(SortOrder::Name),
        "name_nocase" => Ok(SortOrder::NameNoCase),
        "size" => Ok(SortOrder::Size),
        "date" => Ok(SortOrder::Date),
        "natural" => Ok(SortOrder::Natural),
        unknown => {
            eprintln!(
                "警告: default_sort の値 '{unknown}' は無効です。デフォルト(name)を使用します。"
            );
            Ok(SortOrder::Name)
        }
    }
}

// --- Default実装 ---

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            auto_scale: DisplayModeConfig::default(),
            fixed_scale: 1.0,
            margin: 64.0,
            alpha_background: AlphaBackground::default(),
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

impl Default for SusieConfig {
    fn default() -> Self {
        Self {
            plugin_dir: "spi".to_string(),
            image_plugins: Vec::new(),
            archive_plugins: Vec::new(),
        }
    }
}

impl Default for SlideshowConfig {
    fn default() -> Self {
        Self {
            interval_ms: 3000,
            repeat: true,
        }
    }
}

// --- 変換ヘルパー ---

impl DisplayConfig {
    /// DisplayModeConfigからDisplayModeに変換（Fixed + fixed_scaleの組み合わせが必要なため維持）
    pub fn to_display_mode(&self) -> DisplayMode {
        match self.auto_scale {
            DisplayModeConfig::Shrink => DisplayMode::AutoShrink,
            DisplayModeConfig::Fit => DisplayMode::AutoFit,
            DisplayModeConfig::Enlarge => DisplayMode::AutoEnlarge,
            DisplayModeConfig::Original => DisplayMode::Original,
            DisplayModeConfig::Fixed => DisplayMode::Fixed(self.fixed_scale),
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
    /// exeディレクトリの `ぐらびゅ.toml` を読み込む。
    /// ファイルなし / パース失敗はデフォルトにフォールバック。
    pub fn load() -> Self {
        let Some(config_path) = Self::config_path() else {
            return Config::default();
        };
        match Self::load_from(&config_path) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("警告: 設定ファイルの読み込みに失敗しました: {e}");
                Config::default()
            }
        }
    }

    /// 指定パスからConfigを読み込む
    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// 設定ファイルのパスを返す（exeと同じディレクトリの ぐらびゅ.toml）
    fn config_path() -> Option<std::path::PathBuf> {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("ぐらびゅ.toml")))
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = Config::default();
        assert_eq!(config.display.auto_scale, DisplayModeConfig::Fit);
        assert_eq!(config.display.fixed_scale, 1.0);
        assert_eq!(config.display.margin, 64.0);
        assert_eq!(config.display.alpha_background, AlphaBackground::Checker);
        assert_eq!(config.prefetch.cache_base_width, 1024);
        assert_eq!(config.prefetch.cache_base_height, 1536);
        assert_eq!(config.list.default_sort, SortOrder::Name);
        assert!(!config.window.remember_position);
        assert!(!config.window.remember_size);
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
        assert_eq!(config.display.auto_scale, DisplayModeConfig::Fit);
        assert_eq!(config.display.fixed_scale, 2.0);
        assert_eq!(config.display.margin, 10.0);
        assert_eq!(config.display.alpha_background, AlphaBackground::Black);
        assert_eq!(config.prefetch.cache_base_width, 800);
        assert_eq!(config.list.default_sort, SortOrder::Natural);
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
        assert_eq!(config.display.auto_scale, DisplayModeConfig::Original);
        // 未指定フィールドはデフォルト
        assert_eq!(config.display.margin, 64.0);
        assert_eq!(config.list.default_sort, SortOrder::Name);
    }

    #[test]
    fn display_mode_conversion() {
        let d = DisplayConfig {
            auto_scale: DisplayModeConfig::Shrink,
            ..Default::default()
        };
        assert_eq!(d.to_display_mode(), DisplayMode::AutoShrink);

        let d = DisplayConfig {
            auto_scale: DisplayModeConfig::Fit,
            ..Default::default()
        };
        assert_eq!(d.to_display_mode(), DisplayMode::AutoFit);

        let d = DisplayConfig {
            auto_scale: DisplayModeConfig::Fixed,
            fixed_scale: 2.5,
            ..Default::default()
        };
        assert_eq!(d.to_display_mode(), DisplayMode::Fixed(2.5));
    }

    #[test]
    fn invalid_values_fallback_to_defaults() {
        // 無効なauto_scaleはFitにフォールバック
        let toml_str = r#"
[display]
auto_scale = "unknown"
alpha_background = "invalid"

[list]
default_sort = "bogus"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.display.auto_scale, DisplayModeConfig::Fit);
        assert_eq!(config.display.alpha_background, AlphaBackground::Checker);
        assert_eq!(config.list.default_sort, SortOrder::Name);
    }

    #[test]
    fn toml_default_matches_rust_default() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ぐらびゅ.default.toml");
        let config = Config::load_from(&path).expect("ぐらびゅ.default.toml の読み込みに失敗");
        let default = Config::default();

        // display
        assert_eq!(config.display.auto_scale, default.display.auto_scale);
        assert_eq!(config.display.fixed_scale, default.display.fixed_scale);
        assert_eq!(config.display.margin, default.display.margin);
        assert_eq!(
            config.display.alpha_background,
            default.display.alpha_background
        );

        // prefetch
        assert_eq!(
            config.prefetch.cache_base_width,
            default.prefetch.cache_base_width
        );
        assert_eq!(
            config.prefetch.cache_base_height,
            default.prefetch.cache_base_height
        );

        // list
        assert_eq!(config.list.default_sort, default.list.default_sort);

        // window
        assert_eq!(
            config.window.remember_position,
            default.window.remember_position
        );
        assert_eq!(config.window.remember_size, default.window.remember_size);
        assert_eq!(config.window.always_on_top, default.window.always_on_top);

        // susie
        assert_eq!(config.susie.plugin_dir, default.susie.plugin_dir);
        assert_eq!(config.susie.image_plugins, default.susie.image_plugins);
        assert_eq!(config.susie.archive_plugins, default.susie.archive_plugins);

        // slideshow
        assert_eq!(config.slideshow.interval_ms, default.slideshow.interval_ms);
        assert_eq!(config.slideshow.repeat, default.slideshow.repeat);
    }

    #[test]
    fn prefetch_base_image_size() {
        let p = PrefetchConfig::default();
        assert_eq!(p.base_image_size(), 1024 * 1536 * 4);
    }

    #[test]
    fn prefetch_base_image_size_custom() {
        let p = PrefetchConfig {
            cache_base_width: 1920,
            cache_base_height: 1080,
        };
        assert_eq!(p.base_image_size(), 1920 * 1080 * 4);
    }

    #[test]
    fn prefetch_base_image_size_zero() {
        let p = PrefetchConfig {
            cache_base_width: 0,
            cache_base_height: 0,
        };
        assert_eq!(p.base_image_size(), 0);
    }

    #[test]
    fn invalid_auto_scale_fallback() {
        let toml_str = r#"
[display]
auto_scale = "zoom"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.display.auto_scale, DisplayModeConfig::Fit);
    }

    #[test]
    fn invalid_alpha_background_fallback() {
        let toml_str = r#"
[display]
alpha_background = "red"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.display.alpha_background, AlphaBackground::Checker);
    }

    #[test]
    fn invalid_sort_order_fallback() {
        let toml_str = r#"
[list]
default_sort = "random"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.list.default_sort, SortOrder::Name);
    }

    #[test]
    fn all_valid_auto_scale_values() {
        for (value, expected) in [
            ("shrink", DisplayModeConfig::Shrink),
            ("fit", DisplayModeConfig::Fit),
            ("enlarge", DisplayModeConfig::Enlarge),
            ("original", DisplayModeConfig::Original),
            ("fixed", DisplayModeConfig::Fixed),
        ] {
            let toml_str = format!("[display]\nauto_scale = \"{value}\"");
            let config: Config = toml::from_str(&toml_str).unwrap();
            assert_eq!(config.display.auto_scale, expected, "auto_scale={value}");
        }
    }

    #[test]
    fn all_valid_alpha_background_values() {
        for (value, expected) in [
            ("white", AlphaBackground::White),
            ("black", AlphaBackground::Black),
            ("checker", AlphaBackground::Checker),
        ] {
            let toml_str = format!("[display]\nalpha_background = \"{value}\"");
            let config: Config = toml::from_str(&toml_str).unwrap();
            assert_eq!(
                config.display.alpha_background, expected,
                "alpha_background={value}"
            );
        }
    }

    #[test]
    fn all_valid_sort_order_values() {
        for (value, expected) in [
            ("name", SortOrder::Name),
            ("name_nocase", SortOrder::NameNoCase),
            ("size", SortOrder::Size),
            ("date", SortOrder::Date),
            ("natural", SortOrder::Natural),
        ] {
            let toml_str = format!("[list]\ndefault_sort = \"{value}\"");
            let config: Config = toml::from_str(&toml_str).unwrap();
            assert_eq!(config.list.default_sort, expected, "default_sort={value}");
        }
    }

    #[test]
    fn display_mode_conversion_enlarge_and_original() {
        let d = DisplayConfig {
            auto_scale: DisplayModeConfig::Enlarge,
            ..Default::default()
        };
        assert_eq!(d.to_display_mode(), DisplayMode::AutoEnlarge);

        let d = DisplayConfig {
            auto_scale: DisplayModeConfig::Original,
            ..Default::default()
        };
        assert_eq!(d.to_display_mode(), DisplayMode::Original);
    }

    #[test]
    fn empty_toml_uses_all_defaults() {
        let config: Config = toml::from_str("").unwrap();
        let default = Config::default();
        assert_eq!(config.display.auto_scale, default.display.auto_scale);
        assert_eq!(config.display.fixed_scale, default.display.fixed_scale);
        assert_eq!(config.display.margin, default.display.margin);
        assert_eq!(
            config.prefetch.cache_base_width,
            default.prefetch.cache_base_width
        );
        assert_eq!(
            config.prefetch.cache_base_height,
            default.prefetch.cache_base_height
        );
        assert_eq!(config.list.default_sort, default.list.default_sort);
    }

    #[test]
    fn load_from_nonexistent_file_returns_error() {
        let result = Config::load_from(Path::new("nonexistent_file_xyz.toml"));
        assert!(result.is_err());
    }
}

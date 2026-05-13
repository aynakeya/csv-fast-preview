use eframe::egui::{Context, FontData, FontDefinitions, FontFamily};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const CJK_FONT_NAME: &str = "csvfastview_cjk_fallback";

pub(super) fn install_cjk_fallback(ctx: &Context) {
    let Some(path) = find_cjk_font() else {
        return;
    };
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };

    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        CJK_FONT_NAME.to_string(),
        Arc::new(FontData::from_owned(bytes)),
    );

    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push(CJK_FONT_NAME.to_string());
    }

    ctx.set_fonts(fonts);
}

fn find_cjk_font() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CSVFASTVIEW_FONT").map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }

    cjk_font_candidates()
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

fn cjk_font_candidates() -> Vec<&'static Path> {
    vec![
        Path::new("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc"),
        Path::new("/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc"),
        Path::new("/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf"),
        Path::new("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc"),
        Path::new("/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc"),
        Path::new("/usr/share/fonts/opentype/source-han-sans/SourceHanSansCN-Regular.otf"),
        Path::new("/System/Library/Fonts/PingFang.ttc"),
        Path::new("/System/Library/Fonts/STHeiti Light.ttc"),
        Path::new("C:/Windows/Fonts/msyh.ttc"),
        Path::new("C:/Windows/Fonts/simsun.ttc"),
    ]
}

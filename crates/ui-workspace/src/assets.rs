//! The app's asset source: our brand images, the extra Lucide icons we use, then gpui-kit's default
//! component icons. Register it with `application().with_assets(AppAssets)`, or every `Icon` renders blank.

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

pub use gpui_kit::assets::IconName;

icon_assets!(
    ExtraIcons,
    [
        HardDrive,
        Images,
        Copy,
        EyeOff,
        Sparkles,
        FolderOutput,
        ChartColumn,
        Squirrel,
        ScanSearch,
        RotateCw,
        PanelLeft,
        PanelRight,
        Command,
        ShieldCheck,
        Play,
        Hash,
        Camera,
        Aperture,
        FolderPlus,
        Keyboard,
        LayoutGrid,
        Maximize2,
        Layers,
        Activity,
        ArrowRight,
        SlidersHorizontal,
        Monitor,
        Palette,
        Bot,
        ScanLine,
        Folder,
        Sun,
        Moon,
        Settings,
        Search,
        Info,
        TriangleAlert,
        CircleAlert,
        Check,
        X,
        Plus,
        ChevronRight,
        // scan-dashboard / export
        Link,
        Trash,
        Unlink,
        BadgeCheck,
        WifiOff,
        Timer,
        Gauge,
        // duplicate-review / ai-insights
        Scissors,
        Merge,
        Star,
        Users,
        Baby,
        PawPrint,
        Pause,
        Undo2,
        Columns2,
        ZoomIn,
        Crown,
        CircleCheck,
        User,
        Clock,
        Tag,
        KeyRound,
        Plug,
        // preview-placeholders: media glyphs and status marks of placeholder tiles
        Image,
        Film,
        Download,
        RefreshCw,
    ]
);

/// The app mark (64 px PNG, drawn at 24-28 px in the title bar and larger on onboarding).
pub const BRAND_MARK: &str = "brand/photosquirrel-128.png";
const BRAND_MARK_BYTES: &[u8] = include_bytes!("../../../assets/icon/png/128x128.png");

/// Brand images, extra icons, then gpui-kit's icons. Order matters: `Assets` reports an unknown path as an
/// error rather than `None`, so it is tried last.
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == BRAND_MARK {
            return Ok(Some(Cow::Borrowed(BRAND_MARK_BYTES)));
        }
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = Assets.list(path)?;
        all.extend(ExtraIcons.list(path)?);
        if BRAND_MARK.starts_with(path) {
            all.push(BRAND_MARK.into());
        }
        all.sort();
        all.dedup();
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn brand_and_icons_load() {
        for name in [IconName::Images, IconName::Squirrel, IconName::Sparkles] {
            let path = name.path();
            assert!(AppAssets.load(&path).unwrap().is_some(), "{path} missing");
        }
        assert!(AppAssets.load(BRAND_MARK).unwrap().is_some());
    }
}

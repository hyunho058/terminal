//! Resolved sidebar design tokens — colours ([`SidebarTheme`]) and metrics
//! ([`SidebarMetrics`]) — plus the [`AgentSkin`] vocabulary and the packed-`u32`
//! → linear-RGBA [`linear`] conversion. One source of truth for the mock's design
//! values (SD1, D6), so the two consumer features can't drift on a colour or a
//! metric. Filled by the `sidebar-foundations` T3 task.

use common::config::SidebarConfig;

/// Packed `0xRRGGBB` → linear-light RGBA with the given alpha, matching the sRGB
/// surface exactly as the cell/tab-bar path does (this is the surviving copy of the
/// conversion that lived privately in `tab_bar.rs`, which `sidebar-row-redesign`
/// deletes — D6).
pub fn linear(packed: u32, alpha: f32) -> [f32; 4] {
    let [r, g, b] = term_core::Color(packed).to_linear_f32();
    [r, g, b, alpha]
}

/// The mock's sidebar colour tokens, packed `0xRRGGBB`. Every value except `accent`
/// is design (not a user setting); `accent` is threaded in from config (D1/D6) but no
/// sidebar element consumes it yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidebarTheme {
    /// Sidebar panel background (`#1e1b18`).
    pub panel_bg: u32,
    /// 1px right border separating the sidebar from the grid (`#2e2a24`).
    pub panel_border: u32,
    /// Plain card background on hover (`#2a2722`). The unselected plain card has no
    /// fill (transparent) and the unselected border is transparent — so neither is a
    /// stored colour.
    pub card_bg_hover: u32,
    /// Plain card background when selected (`#302c26`).
    pub card_bg_active: u32,
    /// Plain card border when selected (`#46413a`).
    pub card_border_active: u32,
    /// Tab title text (`#e8e2d2`).
    pub title_fg: u32,
    /// Branch / meta sub-line text (`#8f887a`).
    pub meta_fg: u32,
    /// Header/branch icon stroke (`#8f887a`).
    pub icon_fg: u32,
    /// Header icon stroke on hover (`#c9c2b2`).
    pub icon_fg_hover: u32,
    /// Header icon button background on hover (`#2a2722`).
    pub icon_bg_hover: u32,
    /// Agent mark glyph `✳` (`#e2d9c8`).
    pub mark_agent_fg: u32,
    /// Ready mark glyph `◇` (`#7796b8`).
    pub mark_ready_fg: u32,
    /// Waiting-band `⏸` input-waiting signal — amber attention (`#e0a866`, D6).
    pub waiting_fg: u32,
    /// Waiting-band `✓` clean-completion signal — green (`#7fb069`, D6).
    pub completion_fg: u32,
    /// Waiting-band `✕` error signal — red (`#e0574d`, D6).
    pub error_fg: u32,
    /// Waiting-band zone background — a hair lighter than `panel_bg` so the pinned
    /// band reads as a distinct zone (`#232019`, D9).
    pub waiting_band_bg: u32,
    /// Waiting-band bottom separator between the band and the card list (`#3a352d`, D9).
    pub waiting_band_border: u32,
    /// Accent (config `sidebar.accent`, default `#86b0a1`). Stored so the value has a
    /// home; read by no sidebar element in this feature (D1).
    pub accent: u32,
    /// Weather-card title text (`#f4efe6`, sidebar-weather-cards D9).
    pub card_title_fg: u32,
    /// Weather-card secondary text — folder, branch, `zsh`, `Running` — drawn at
    /// [`CARD_META_ALPHA`] (`#f4efe6` @ .78, D9).
    pub card_meta_fg: u32,
    /// `Working` status (`#ffd9bd`, D9) — every skin except Codex.
    pub status_working_fg: u32,
    /// `Working` status on a Codex card (`#c9ecdf`, D9).
    pub status_working_codex_fg: u32,
    /// `Ready` status (`#b9cdf0`, D9).
    pub status_ready_fg: u32,
    /// `Waiting for input` status (`#f2c186`, D9).
    pub status_waiting_fg: u32,
    /// `Exited N` status (`#ff9a90`, D9).
    pub status_error_fg: u32,
}

/// Alpha of [`SidebarTheme::card_meta_fg`] — the mock's `rgba(244,239,230,0.78)`.
pub const CARD_META_ALPHA: f32 = 0.78;
/// Alpha of the white 1px border on a default card (`rgba(255,255,255,.10)`, D4).
pub const CARD_BORDER_ALPHA: f32 = 0.10;
/// Alpha of the white 1px border on a hovered card (`rgba(255,255,255,.18)`, D4).
pub const CARD_BORDER_HOVER_ALPHA: f32 = 0.18;
/// Alpha of the `≡` button's white backplate (`rgba(255,255,255,.16)`, D11).
pub const MENU_BACKPLATE_ALPHA: f32 = 0.16;

impl SidebarTheme {
    /// Every colour token at the mock's exact value; `accent` at the config default.
    pub const DEFAULT: SidebarTheme = SidebarTheme {
        panel_bg: 0x001E_1B18,
        panel_border: 0x002E_2A24,
        card_bg_hover: 0x002A_2722,
        card_bg_active: 0x0030_2C26,
        card_border_active: 0x0046_413A,
        title_fg: 0x00E8_E2D2,
        meta_fg: 0x008F_887A,
        icon_fg: 0x008F_887A,
        icon_fg_hover: 0x00C9_C2B2,
        icon_bg_hover: 0x002A_2722,
        mark_agent_fg: 0x00E2_D9C8,
        mark_ready_fg: 0x0077_96B8,
        waiting_fg: 0x00E0_A866,
        completion_fg: 0x007F_B069,
        error_fg: 0x00E0_574D,
        waiting_band_bg: 0x0023_2019,
        waiting_band_border: 0x003A_352D,
        accent: 0x0086_B0A1,
        card_title_fg: 0x00F4_EFE6,
        card_meta_fg: 0x00F4_EFE6,
        status_working_fg: 0x00FF_D9BD,
        status_working_codex_fg: 0x00C9_ECDF,
        status_ready_fg: 0x00B9_CDF0,
        status_waiting_fg: 0x00F2_C186,
        status_error_fg: 0x00FF_9A90,
    };

    /// Resolve the theme from config: [`DEFAULT`] with the one config-derived colour
    /// (`accent`) overlaid (D6). The layout toggles (`show_git_branch` /
    /// `show_agent_status`) are deliberately NOT stored here — they are not colours,
    /// and consumers read them from `FrameCtx.sidebar` directly.
    ///
    /// [`DEFAULT`]: SidebarTheme::DEFAULT
    pub fn from_config(cfg: &SidebarConfig) -> SidebarTheme {
        SidebarTheme {
            accent: cfg.accent,
            ..SidebarTheme::DEFAULT
        }
    }
}

/// The mock's sidebar metric tokens, in **logical px**. Call [`scaled`] once with the
/// window backing factor to get physical px — never multiply a field ad hoc, so the
/// app's hit-testing and the renderer's drawing can't disagree (SD2/D6).
///
/// [`scaled`]: SidebarMetrics::scaled
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarMetrics {
    pub card_pad_x: f32,
    pub card_gap: f32,
    pub border_w: f32,
    pub header_pad_top: f32,
    pub header_pad_x: f32,
    pub header_pad_bottom: f32,
    pub header_gap: f32,
    pub button_pad: f32,
    pub button_radius: f32,
    pub icon_stroke_w: f32,
    pub mark_px: f32,
    pub title_px: f32,
    pub meta_px: f32,
}

impl SidebarMetrics {
    /// The mock's values, in logical px.
    pub const DEFAULT: SidebarMetrics = SidebarMetrics {
        card_pad_x: 15.0,
        card_gap: 6.0,
        border_w: 1.0,
        header_pad_top: 14.0,
        header_pad_x: 12.0,
        header_pad_bottom: 12.0,
        header_gap: 8.0,
        button_pad: 6.0,
        button_radius: 8.0,
        icon_stroke_w: 1.7,
        mark_px: 13.0,
        title_px: 15.0,
        meta_px: 12.5,
    };

    /// Every metric multiplied by `scale` — the single conversion site from logical
    /// to physical px (D6). Centralises the multiply so no call site can forget it.
    pub fn scaled(&self, scale: f64) -> SidebarMetrics {
        let s = scale as f32;
        SidebarMetrics {
            card_pad_x: self.card_pad_x * s,
            card_gap: self.card_gap * s,
            border_w: self.border_w * s,
            header_pad_top: self.header_pad_top * s,
            header_pad_x: self.header_pad_x * s,
            header_pad_bottom: self.header_pad_bottom * s,
            header_gap: self.header_gap * s,
            button_pad: self.button_pad * s,
            button_radius: self.button_radius * s,
            icon_stroke_w: self.icon_stroke_w * s,
            mark_px: self.mark_px * s,
            title_px: self.title_px * s,
            meta_px: self.meta_px * s,
        }
    }
}

/// The weather card's metric tokens (sidebar-weather-cards D1/D2/D6/D11), in **logical
/// px**. A separate struct from [`SidebarMetrics`] on purpose: the waiting band, the
/// header and the menu popup still read that one, and they are out of this feature's
/// scope, so changing a shared value there would repaint them. Call [`scaled`] once per
/// frame, exactly as for [`SidebarMetrics`].
///
/// [`scaled`]: CardMetrics::scaled
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardMetrics {
    /// Card height — fixed, independent of the terminal font (D1).
    pub height: f32,
    pub radius: f32,
    pub pad_x: f32,
    pub pad_y: f32,
    /// Card inset from each sidebar edge (the mock's list padding).
    pub inset_x: f32,
    /// Vertical gap between consecutive cards.
    pub gap: f32,
    /// Horizontal gap between the title column and the agent icon, and between the
    /// bottom row's status and branch.
    pub zone_gap: f32,
    /// Border width, default and hover.
    pub border: f32,
    /// Border width when selected.
    pub border_selected: f32,
    /// Selected-state glow ring thickness (the mock's `box-shadow: 0 0 0 3px`).
    pub glow: f32,
    /// Title text size (17px bold, D2).
    pub title_px: f32,
    /// Folder / status / branch text size (12.5px, D2).
    pub meta_px: f32,
    /// Vertical gap between the title line and the folder line.
    pub title_gap: f32,
    /// Top offset of the Claude icon inside the padding box.
    pub icon_top_claude: f32,
    /// Top offset of every other agent icon inside the padding box.
    pub icon_top_other: f32,
    /// The `≡` button's backplate side (D11).
    pub menu_button: f32,
    pub menu_radius: f32,
    /// Gap between the `≡` backplate and the agent icon.
    pub menu_gap: f32,
    /// The hamburger glyph's size inside the backplate.
    pub menu_glyph: f32,
    /// Gap between a zone icon and its text (status, folder).
    pub icon_text_gap: f32,
    /// Gap between the branch icon and the branch name (the mock's 5px).
    pub branch_icon_gap: f32,
    /// The `≡` backplate's top offset inside the padding box: the 26px button centred
    /// against a 30px-tall icon row (`align-items: center`).
    pub menu_top: f32,
    /// D21's 12px zone icons: Ready, Waiting, Exited and the branch.
    pub zone_icon: f32,
    /// D21's 13px zone icons: Working, `Running` / `zsh` and the folder.
    pub zone_icon_lg: f32,
}

impl CardMetrics {
    /// The mock's values, logical px (Anatomy board + After board list metrics).
    pub const DEFAULT: CardMetrics = CardMetrics {
        height: 104.0,
        radius: 16.0,
        pad_x: 16.0,
        pad_y: 12.0,
        inset_x: 14.0,
        gap: 10.0,
        zone_gap: 10.0,
        border: 1.0,
        border_selected: 1.5,
        glow: 3.0,
        title_px: 17.0,
        meta_px: 12.5,
        title_gap: 3.0,
        icon_top_claude: 4.0,
        icon_top_other: 2.0,
        menu_button: 26.0,
        menu_radius: 8.0,
        menu_gap: 8.0,
        menu_glyph: 14.0,
        icon_text_gap: 6.0,
        branch_icon_gap: 5.0,
        menu_top: 3.0,
        zone_icon: 12.0,
        zone_icon_lg: 13.0,
    };

    /// Every metric multiplied by `scale` — the one logical → physical conversion.
    pub fn scaled(&self, scale: f64) -> CardMetrics {
        let s = scale as f32;
        CardMetrics {
            height: self.height * s,
            radius: self.radius * s,
            pad_x: self.pad_x * s,
            pad_y: self.pad_y * s,
            inset_x: self.inset_x * s,
            gap: self.gap * s,
            zone_gap: self.zone_gap * s,
            border: self.border * s,
            border_selected: self.border_selected * s,
            glow: self.glow * s,
            title_px: self.title_px * s,
            meta_px: self.meta_px * s,
            title_gap: self.title_gap * s,
            icon_top_claude: self.icon_top_claude * s,
            icon_top_other: self.icon_top_other * s,
            menu_button: self.menu_button * s,
            menu_radius: self.menu_radius * s,
            menu_gap: self.menu_gap * s,
            menu_glyph: self.menu_glyph * s,
            icon_text_gap: self.icon_text_gap * s,
            branch_icon_gap: self.branch_icon_gap * s,
            menu_top: self.menu_top * s,
            zone_icon: self.zone_icon * s,
            zone_icon_lg: self.zone_icon_lg * s,
        }
    }
}

/// A gradient fill: up to three linear-RGBA stops plus a mid-stop position and a
/// direction unit-vector in local box space. `stops[2] == stops[1]` → a 2-stop
/// gradient; all three equal → a flat fill. Matches what the shape primitive (T4)
/// consumes per instance (D4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinFill {
    pub stops: [[f32; 4]; 3],
    pub mid: f32,
    pub dir: [f32; 2],
}

/// Which agent a tab is running, mapped to a card skin. Total and exhaustive: an
/// agent identity the mapping does not recognise resolves to [`Generic`], the plain
/// card treatment — so an unknown agent renders as a normal tab rather than failing
/// (D6/D13). `sidebar-row-redesign` owns the identity → skin mapping; this feature
/// only defines the vocabulary and the resolved colours.
///
/// [`Generic`]: AgentSkin::Generic
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSkin {
    Claude,
    Antigravity,
    /// OpenAI Codex (sidebar-weather-cards D5) — the dark-teal card.
    Codex,
    Generic,
}

/// The weather card's gradient direction: CSS `linear-gradient(160deg, …)` as a unit
/// vector in top-left-origin space — `(sin 160°, −cos 160°)` (D5). CSS 0° points up and
/// angles run clockwise, so 160° is mostly down and a little right.
pub const DIR_160: [f32; 2] = [0.342_020_14, 0.939_692_6];

/// Where the middle stop sits on every card gradient (`… b 55%, …`).
const CARD_MID: f32 = 0.55;

/// A weather card's full palette (D4/D5): the gradient for the default state, the
/// brighter one for hover **and** selected, the selected border tint and the selected
/// glow ring. All linear RGBA.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardSkin {
    pub default: SkinFill,
    pub hover: SkinFill,
    /// The 1.5px selected-border colour.
    pub tint: [f32; 4],
    /// The 3px glow ring drawn behind a selected card.
    pub glow: [f32; 4],
}

/// An opaque three-stop card gradient at the mock's 160° / 55%.
fn card_fill(a: u32, b: u32, c: u32) -> SkinFill {
    SkinFill {
        stops: [linear(a, 1.0), linear(b, 1.0), linear(c, 1.0)],
        mid: CARD_MID,
        dir: DIR_160,
    }
}

impl AgentSkin {
    /// The weather card's palette for this skin — the Anatomy board's values
    /// (sidebar-weather-cards D5, the L1 table). `Generic` is the plain-shell card.
    pub fn card(self) -> CardSkin {
        match self {
            AgentSkin::Claude => CardSkin {
                default: card_fill(0x007A_3E22, 0x004E_2818, 0x0033_1C14),
                hover: card_fill(0x008A_4526, 0x005A_2C1A, 0x003A_1F16),
                tint: linear(0x00FF_D6BA, 0.62),
                glow: linear(0x00FF_BE96, 0.13),
            },
            AgentSkin::Antigravity => CardSkin {
                default: card_fill(0x0026_4578, 0x0035_2866, 0x0047_1E26),
                hover: card_fill(0x002C_4F86, 0x003D_2F74, 0x0052_232C),
                tint: linear(0x00C4_D4FF, 0.60),
                glow: linear(0x00A0_B4FF, 0.14),
            },
            AgentSkin::Codex => CardSkin {
                default: card_fill(0x0024_403B, 0x001E_2D2A, 0x0019_201E),
                hover: card_fill(0x002A_4A44, 0x0022_332F, 0x001C_2422),
                tint: linear(0x00C8_ECDF, 0.60),
                glow: linear(0x00A0_DCC8, 0.13),
            },
            AgentSkin::Generic => CardSkin {
                default: card_fill(0x003D_3934, 0x002E_2B27, 0x0025_221F),
                hover: card_fill(0x0046_413B, 0x0034_302B, 0x0029_2622),
                tint: linear(0x00FF_FFFF, 0.50),
                glow: linear(0x00FF_FFFF, 0.10),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_default_carries_mock_values() {
        // R6.1 — every colour token present at the mock's exact value.
        let t = SidebarTheme::DEFAULT;
        assert_eq!(t.panel_bg, 0x001E_1B18);
        assert_eq!(t.panel_border, 0x002E_2A24);
        assert_eq!(t.card_bg_hover, 0x002A_2722);
        assert_eq!(t.card_bg_active, 0x0030_2C26);
        assert_eq!(t.card_border_active, 0x0046_413A);
        assert_eq!(t.title_fg, 0x00E8_E2D2);
        assert_eq!(t.meta_fg, 0x008F_887A);
        assert_eq!(t.mark_agent_fg, 0x00E2_D9C8);
        assert_eq!(t.mark_ready_fg, 0x0077_96B8);
        assert_eq!(t.accent, 0x0086_B0A1);
    }

    #[test]
    fn from_config_overlays_accent_only() {
        // R6.2 — from_config changes accent and nothing else.
        let cfg = SidebarConfig {
            accent: 0x00E0_863F,
            ..SidebarConfig::default()
        };
        let t = SidebarTheme::from_config(&cfg);
        assert_eq!(t.accent, 0x00E0_863F);
        assert_eq!(
            SidebarTheme {
                accent: SidebarTheme::DEFAULT.accent,
                ..t
            },
            SidebarTheme::DEFAULT,
            "no non-accent colour may change"
        );
    }

    #[test]
    fn linear_matches_srgb_conversion() {
        // R6.3 — packed → linear RGBA, alpha passed through.
        let got = linear(0x0086_B0A1, 1.0);
        let [r, g, b] = term_core::Color(0x0086_B0A1).to_linear_f32();
        assert_eq!(got, [r, g, b, 1.0]);
        assert_eq!(linear(0x00FF_FFFF, 0.5)[3], 0.5, "alpha is passed through");
    }

    #[test]
    fn metrics_default_are_mock_values() {
        // R6.4 — mock's numbers, unscaled.
        let m = SidebarMetrics::DEFAULT;
        assert_eq!(m.card_pad_x, 15.0);
        assert_eq!(m.card_gap, 6.0);
        assert_eq!(m.border_w, 1.0);
    }

    #[test]
    fn scaled_doubles_every_metric() {
        // R6.5 — scaled(2.0) multiplies every field exactly once. Asserting field by
        // field so a metric added later without scaling fails this test.
        let d = SidebarMetrics::DEFAULT;
        let s = d.scaled(2.0);
        assert_eq!(s.card_pad_x, d.card_pad_x * 2.0);
        assert_eq!(s.card_gap, d.card_gap * 2.0);
        assert_eq!(s.border_w, d.border_w * 2.0);
        assert_eq!(s.header_pad_top, d.header_pad_top * 2.0);
        assert_eq!(s.header_pad_x, d.header_pad_x * 2.0);
        assert_eq!(s.header_pad_bottom, d.header_pad_bottom * 2.0);
        assert_eq!(s.header_gap, d.header_gap * 2.0);
        assert_eq!(s.button_pad, d.button_pad * 2.0);
        assert_eq!(s.button_radius, d.button_radius * 2.0);
        assert_eq!(s.icon_stroke_w, d.icon_stroke_w * 2.0);
        assert_eq!(s.mark_px, d.mark_px * 2.0);
        assert_eq!(s.title_px, d.title_px * 2.0);
        assert_eq!(s.meta_px, d.meta_px * 2.0);
    }

    // -----------------------------------------------------------------------
    // sidebar-weather-cards — card skins, role tokens, card metrics
    // -----------------------------------------------------------------------

    const ALL_SKINS: [AgentSkin; 4] = [
        AgentSkin::Claude,
        AgentSkin::Antigravity,
        AgentSkin::Codex,
        AgentSkin::Generic,
    ];

    /// One row of the design's L1 table: default stops, hover stops, tint, glow (each
    /// colour + alpha).
    type DesignRow = ([u32; 3], [u32; 3], (u32, f32), (u32, f32));

    /// The design's L1 table, verbatim.
    fn design(skin: AgentSkin) -> DesignRow {
        match skin {
            AgentSkin::Claude => (
                [0x7A3E22, 0x4E2818, 0x331C14],
                [0x8A4526, 0x5A2C1A, 0x3A1F16],
                (0xFFD6BA, 0.62),
                (0xFFBE96, 0.13),
            ),
            AgentSkin::Antigravity => (
                [0x264578, 0x352866, 0x471E26],
                [0x2C4F86, 0x3D2F74, 0x52232C],
                (0xC4D4FF, 0.60),
                (0xA0B4FF, 0.14),
            ),
            AgentSkin::Codex => (
                [0x24403B, 0x1E2D2A, 0x19201E],
                [0x2A4A44, 0x22332F, 0x1C2422],
                (0xC8ECDF, 0.60),
                (0xA0DCC8, 0.13),
            ),
            AgentSkin::Generic => (
                [0x3D3934, 0x2E2B27, 0x25221F],
                [0x46413B, 0x34302B, 0x292622],
                (0xFFFFFF, 0.50),
                (0xFFFFFF, 0.10),
            ),
        }
    }

    #[test]
    fn card_skins_equal_the_design_table() {
        // R5.2 — every stop, the mid, the 160° direction, the tint and the glow.
        for skin in ALL_SKINS {
            let c = skin.card();
            let (def, hov, (tint, ta), (glow, ga)) = design(skin);
            for (fill, want) in [(c.default, def), (c.hover, hov)] {
                for (i, hex) in want.iter().enumerate() {
                    assert_eq!(fill.stops[i], linear(*hex, 1.0), "{skin:?} stop {i}");
                }
                assert_eq!(fill.mid, 0.55, "{skin:?} mid stop");
                assert!((fill.dir[0] - 0.3420).abs() < 1e-4, "{skin:?} dir x");
                assert!((fill.dir[1] - 0.9397).abs() < 1e-4, "{skin:?} dir y");
            }
            assert_eq!(c.tint, linear(tint, ta), "{skin:?} tint");
            assert_eq!(c.glow, linear(glow, ga), "{skin:?} glow");
        }
    }

    #[test]
    fn dir_160_is_a_unit_vector_pointing_mostly_down() {
        let [x, y] = DIR_160;
        assert!((x * x + y * y - 1.0).abs() < 1e-6, "unit length");
        assert!(y > x && x > 0.0, "160° CSS: mostly down, a little right");
    }

    #[test]
    fn card_skins_are_pairwise_distinct() {
        for (i, a) in ALL_SKINS.iter().enumerate() {
            for b in &ALL_SKINS[i + 1..] {
                assert_ne!(a.card(), b.card(), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn card_role_tokens_hold_the_mock_values() {
        // R9.1 — the new role tokens and their alphas.
        let t = SidebarTheme::DEFAULT;
        assert_eq!(t.card_title_fg, 0x00F4_EFE6);
        assert_eq!(t.card_meta_fg, 0x00F4_EFE6);
        assert_eq!(CARD_META_ALPHA, 0.78);
        assert_eq!(t.status_working_fg, 0x00FF_D9BD);
        assert_eq!(t.status_working_codex_fg, 0x00C9_ECDF);
        assert_eq!(t.status_ready_fg, 0x00B9_CDF0);
        assert_eq!(t.status_waiting_fg, 0x00F2_C186);
        assert_eq!(t.status_error_fg, 0x00FF_9A90);
        assert_eq!(CARD_BORDER_ALPHA, 0.10);
        assert_eq!(CARD_BORDER_HOVER_ALPHA, 0.18);
        assert_eq!(MENU_BACKPLATE_ALPHA, 0.16);
    }

    #[test]
    fn card_metrics_default_are_the_anatomy_values() {
        // D1 — "316 × 104 logical px · radius 16 · padding 12 / 16 · gap 10".
        let m = CardMetrics::DEFAULT;
        assert_eq!(m.height, 104.0);
        assert_eq!(m.radius, 16.0);
        assert_eq!((m.pad_y, m.pad_x), (12.0, 16.0));
        assert_eq!((m.inset_x, m.gap, m.zone_gap), (14.0, 10.0, 10.0));
        assert_eq!((m.border, m.border_selected, m.glow), (1.0, 1.5, 3.0));
        assert_eq!((m.title_px, m.meta_px, m.title_gap), (17.0, 12.5, 3.0));
        assert_eq!((m.icon_top_claude, m.icon_top_other), (4.0, 2.0));
        assert_eq!(
            (m.menu_button, m.menu_radius, m.menu_gap, m.menu_glyph),
            (26.0, 8.0, 8.0, 14.0)
        );
        assert_eq!((m.icon_text_gap, m.branch_icon_gap), (6.0, 5.0));
        // D11's button top and D21's two icon sizes.
        assert_eq!((m.menu_top, m.zone_icon, m.zone_icon_lg), (3.0, 12.0, 13.0));
    }

    #[test]
    fn card_metrics_scaled_multiplies_every_field() {
        // Field by field, so a metric added later without scaling fails here. At 4×:
        // a field `x` cannot tell `x·s` from `x+s` when `x = s/(s−1)`, which at 2× is
        // `icon_top_other` (2.0) and at 3× `border_selected` (1.5). At 4× it is 1.33,
        // which no metric holds.
        let d = CardMetrics::DEFAULT;
        let s = d.scaled(4.0);
        let pairs = [
            (s.height, d.height),
            (s.radius, d.radius),
            (s.pad_x, d.pad_x),
            (s.pad_y, d.pad_y),
            (s.inset_x, d.inset_x),
            (s.gap, d.gap),
            (s.zone_gap, d.zone_gap),
            (s.border, d.border),
            (s.border_selected, d.border_selected),
            (s.glow, d.glow),
            (s.title_px, d.title_px),
            (s.meta_px, d.meta_px),
            (s.title_gap, d.title_gap),
            (s.icon_top_claude, d.icon_top_claude),
            (s.icon_top_other, d.icon_top_other),
            (s.menu_button, d.menu_button),
            (s.menu_radius, d.menu_radius),
            (s.menu_gap, d.menu_gap),
            (s.menu_glyph, d.menu_glyph),
            (s.icon_text_gap, d.icon_text_gap),
            (s.branch_icon_gap, d.branch_icon_gap),
            (s.menu_top, d.menu_top),
            (s.zone_icon, d.zone_icon),
            (s.zone_icon_lg, d.zone_icon_lg),
        ];
        for (i, (got, base)) in pairs.iter().enumerate() {
            assert_eq!(*got, base * 4.0, "field #{i} not scaled");
        }
    }
}

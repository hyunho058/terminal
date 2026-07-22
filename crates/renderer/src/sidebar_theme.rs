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
}

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
    pub card_radius: f32,
    pub card_pad_x: f32,
    pub card_pad_y: f32,
    pub card_margin_x: f32,
    pub card_margin_y: f32,
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
        card_radius: 13.0,
        card_pad_x: 15.0,
        card_pad_y: 12.0,
        card_margin_x: 8.0,
        card_margin_y: 3.0,
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
            card_radius: self.card_radius * s,
            card_pad_x: self.card_pad_x * s,
            card_pad_y: self.card_pad_y * s,
            card_margin_x: self.card_margin_x * s,
            card_margin_y: self.card_margin_y * s,
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

/// The four resolved states of an agent card's skin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkinStates {
    /// Card background when not selected.
    pub bg: SkinFill,
    /// Card background when selected / hovered (brighter).
    pub hi: SkinFill,
    /// Card border (linear RGBA).
    pub border: [f32; 4],
    /// A dim tint for secondary emphasis (linear RGBA).
    pub dim: [f32; 4],
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
    Generic,
}

/// Down-right gradient direction (~135°) in top-left-origin box space, normalised.
const DIR_135: [f32; 2] = [0.707_106_77, 0.707_106_77];

/// A flat 3-equal-stop fill of one linear-RGBA colour.
const fn flat(c: [f32; 4]) -> SkinFill {
    SkinFill {
        stops: [c, c, c],
        mid: 0.5,
        dir: DIR_135,
    }
}

impl AgentSkin {
    /// Resolve the four skin states to linear RGBA (D6). `Claude` and `Antigravity`
    /// carry the mock's gradients + borders; `Generic` is the plain-card treatment
    /// (transparent bg → `#2a2722` hover → `#302c26` selected, border `#46413a`).
    pub fn states(self) -> SkinStates {
        match self {
            AgentSkin::Antigravity => SkinStates {
                // rgba(58,134,224,.14) → rgba(122,79,214,.13) @55% → rgba(224,69,58,.11)
                bg: SkinFill {
                    stops: [
                        linear(0x003A_86E0, 0.14),
                        linear(0x007A_4FD6, 0.13),
                        linear(0x00E0_453A, 0.11),
                    ],
                    mid: 0.55,
                    dir: DIR_135,
                },
                // hi: same colours at .32 / .27 / .22
                hi: SkinFill {
                    stops: [
                        linear(0x003A_86E0, 0.32),
                        linear(0x007A_4FD6, 0.27),
                        linear(0x00E0_453A, 0.22),
                    ],
                    mid: 0.55,
                    dir: DIR_135,
                },
                border: linear(0x007A_4FD6, 0.45),
                dim: linear(0x00A0_96DC, 0.16),
            },
            AgentSkin::Claude => SkinStates {
                // rgba(255,157,92,.15) → rgba(214,82,31,.12)  (2-stop → stops[2]==stops[1])
                bg: SkinFill {
                    stops: [
                        linear(0x00FF_9D5C, 0.15),
                        linear(0x00D6_521F, 0.12),
                        linear(0x00D6_521F, 0.12),
                    ],
                    mid: 0.5,
                    dir: DIR_135,
                },
                hi: SkinFill {
                    stops: [
                        linear(0x00FF_9D5C, 0.32),
                        linear(0x00D6_521F, 0.26),
                        linear(0x00D6_521F, 0.26),
                    ],
                    mid: 0.5,
                    dir: DIR_135,
                },
                border: linear(0x00E0_783C, 0.5),
                dim: linear(0x00E0_8C5A, 0.18),
            },
            AgentSkin::Generic => SkinStates {
                // Plain card: transparent unselected, brighter on hover/select.
                bg: flat(linear(0x0000_0000, 0.0)),
                hi: flat(linear(SidebarTheme::DEFAULT.card_bg_active, 1.0)),
                border: linear(SidebarTheme::DEFAULT.card_border_active, 1.0),
                dim: linear(0x0000_0000, 0.0),
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
        assert_eq!(m.card_radius, 13.0);
        assert_eq!(m.card_pad_x, 15.0);
        assert_eq!(m.card_pad_y, 12.0);
        assert_eq!(m.card_margin_x, 8.0);
        assert_eq!(m.card_margin_y, 3.0);
        assert_eq!(m.card_gap, 6.0);
        assert_eq!(m.border_w, 1.0);
    }

    #[test]
    fn scaled_doubles_every_metric() {
        // R6.5 — scaled(2.0) multiplies every field exactly once. Asserting field by
        // field so a metric added later without scaling fails this test.
        let d = SidebarMetrics::DEFAULT;
        let s = d.scaled(2.0);
        assert_eq!(s.card_radius, d.card_radius * 2.0);
        assert_eq!(s.card_pad_x, d.card_pad_x * 2.0);
        assert_eq!(s.card_pad_y, d.card_pad_y * 2.0);
        assert_eq!(s.card_margin_x, d.card_margin_x * 2.0);
        assert_eq!(s.card_margin_y, d.card_margin_y * 2.0);
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

    #[test]
    fn agent_skin_is_total_and_generic_is_plain_card() {
        // R6.6 — every variant resolves; Generic is the plain-card treatment.
        for skin in [
            AgentSkin::Claude,
            AgentSkin::Antigravity,
            AgentSkin::Generic,
        ] {
            let _ = skin.states(); // must not panic for any variant
        }
        let g = AgentSkin::Generic.states();
        // Unselected plain card is transparent (alpha 0).
        assert_eq!(g.bg.stops[0][3], 0.0);
        // Selected uses #302c26; border uses #46413a.
        assert_eq!(g.hi.stops[0], linear(0x0030_2C26, 1.0));
        assert_eq!(g.border, linear(0x0046_413A, 1.0));
    }

    #[test]
    fn agent_skins_are_distinct() {
        // Claude and Antigravity must not resolve to the same skin.
        assert_ne!(AgentSkin::Claude.states(), AgentSkin::Antigravity.states());
        assert_ne!(AgentSkin::Claude.states(), AgentSkin::Generic.states());
    }
}

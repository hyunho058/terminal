//! The Agent Monitor's palette (session-overview-ui D21): the design's colours, in one
//! place, packed `0xRRGGBB`.
//!
//! The design's translucent fills (`rgba(…, .10)` and the like) are listed here already
//! composited over the surface they sit on, so every fill the panel draws is opaque and a
//! golden frame can assert it exactly (R9.5). Each such constant names its source.
//!
//! Two colour spaces cross this module. Layout output that tests compare against the
//! design — [`LaidSquare::color`](crate::overview_layout::LaidSquare) and
//! [`LaidText::color`](crate::overview_layout::LaidText) — is **sRGB** straight RGBA
//! ([`srgb`]); what reaches the GPU is **linear** ([`linear`], [`to_linear`]), because the
//! render target is an sRGB format that encodes on store.

use crate::overview_model::{AgentTag, Bucket, SegmentKind, SubagentStatus};

/// Window backdrop.
pub const BG: u32 = 0x1A1816;
/// The sessions table and the timeline section.
pub const PANEL: u32 = 0x211F1C;
/// Tiles, the filter group, the detail's header card and side cards.
pub const CARD: u32 = 0x24221F;
pub const BORDER: u32 = 0x36322E;
pub const TEXT: u32 = 0xEDE8E1;
pub const MUTED: u32 = 0xA39C92;
/// Uppercase labels and column headers.
pub const LABEL: u32 = 0x9A938A;
/// The quoted prompt and tick labels.
pub const PROMPT: u32 = 0x8F887F;
/// `—` placeholders and the tree's `└`.
pub const FAINT: u32 = 0x6F6961;
/// The Elapsed column and the button icons.
pub const SOFT: u32 = 0xCFC8BE;
/// Bright text: the selected filter, project names in detail, the selected lane label.
pub const BRIGHT: u32 = 0xF4EFE8;
pub const ACCENT: u32 = 0xE8744F;
pub const WAITING: u32 = 0xF2B84B;
pub const WAITING_TEXT: u32 = 0xF2C76B;
pub const DONE: u32 = 0x6FA67F;
pub const FAILED: u32 = 0xE5534B;
/// The lighter green: `Task done · …` events, the selected lane, the done count.
pub const DONE_LIGHT: u32 = 0x8FC29E;

/// The Needs-you tile: `rgba(242,184,75,.10)` over [`BG`], drawn opaque (D21).
pub const NEEDS_YOU_FILL: u32 = 0x30271B;
/// The Needs-you tile's border: `rgba(242,184,75,.45)` over [`BG`] (D21).
pub const NEEDS_YOU_BORDER: u32 = 0x7B602E;
pub const NEEDS_YOU_LABEL: u32 = WAITING_TEXT;
pub const NEEDS_YOU_NUMBER: u32 = 0xFFF3DC;
pub const NEEDS_YOU_CAPTION: u32 = 0xD9C9A8;
pub const WORKING_LABEL: u32 = 0xF09A78;
pub const IDLE_LABEL: u32 = 0xB8B1A7;
pub const IDLE_DOT: u32 = 0x8A837A;

/// The banner: `#2E2717`, bordered `rgba(242,184,75,.4)` over it.
pub const BANNER_BG: u32 = 0x2E2717;
pub const BANNER_BORDER: u32 = 0x7C612C;
/// Text on a waiting-coloured fill (the banner button, the Needs-you pill).
pub const ON_WAITING: u32 = 0x2A2110;
/// Text on the accent fill (`Focus terminal`).
pub const ON_ACCENT: u32 = 0x1E140F;

pub const FILTER_SELECTED: u32 = 0x3A3531;
/// A row of the table: separator under it.
pub const ROW_SEPARATOR: u32 = 0x2E2B28;
/// A Needs-you row: `rgba(242,184,75,.06)` over [`PANEL`].
pub const ROW_NEEDS_YOU: u32 = 0x2D281F;
/// The highlighted row and the selected lane.
pub const ROW_SELECTED: u32 = 0x2D2A26;
pub const BUTTON: u32 = 0x2A2724;
pub const BUTTON_BORDER: u32 = 0x3A3632;
/// Agent tag outline and the tree's rule.
pub const TAG_BORDER: u32 = 0x3F3A35;
/// The breadcrumb link.
pub const LINK: u32 = 0xF0A27F;
/// The subagent report box.
pub const REPORT_BG: u32 = 0x1E1C1A;
/// Timeline track and grid lines.
pub const TRACK: u32 = 0x2A2724;
pub const GRID_LINE: u32 = 0x2C2926;
pub const WAITING_ON_SUBAGENT: u32 = 0x6B3A2A;
pub const IDLE_SEGMENT: u32 = 0x3A3632;

/// A status pill's `(text, fill, dot)` (D2). Translucent design fills are composited over
/// [`PANEL`]: Working `rgba(232,116,79,.16)`, Idle `rgba(163,156,146,.12)`.
pub fn pill(bucket: Bucket) -> (u32, u32, u32) {
    match bucket {
        Bucket::NeedsYou => (ON_WAITING, WAITING, ON_WAITING),
        Bucket::Working => (0xF7B9A0, 0x412D24, ACCENT),
        Bucket::Idle => (IDLE_LABEL, 0x312E2A, IDLE_DOT),
        Bucket::Agent => (MUTED, 0x312E2A, FAINT),
    }
}

/// The pill's label (D2).
pub fn bucket_label(bucket: Bucket) -> &'static str {
    match bucket {
        Bucket::NeedsYou => "Needs you",
        Bucket::Working => "Working",
        Bucket::Idle => "Idle",
        Bucket::Agent => "Agent",
    }
}

/// The Now doing line's colour by bucket.
pub fn activity(bucket: Bucket) -> u32 {
    match bucket {
        Bucket::NeedsYou => WAITING_TEXT,
        Bucket::Working => TEXT,
        Bucket::Idle | Bucket::Agent => MUTED,
    }
}

/// The agent tag's label colour (D3, D21).
pub fn agent(tag: AgentTag) -> u32 {
    match tag {
        AgentTag::Claude => 0xE9967A,
        AgentTag::Codex => 0xA9B8FF,
        AgentTag::Antigravity => 0x86D1C4,
        AgentTag::Gemini | AgentTag::Aider => 0xB8B1A7,
    }
}

/// A square's, tree dot's and lane bar's colour (D8, D12).
pub fn subagent(status: SubagentStatus) -> u32 {
    match status {
        SubagentStatus::Running => ACCENT,
        SubagentStatus::Done => DONE,
        SubagentStatus::Failed => FAILED,
    }
}

/// A main-lane segment's colour; `None` for [`SegmentKind::Blank`], which shows the track.
pub fn segment(kind: SegmentKind) -> Option<u32> {
    match kind {
        SegmentKind::Working => Some(ACCENT),
        SegmentKind::WaitingOnSubagent => Some(WAITING_ON_SUBAGENT),
        SegmentKind::Idle => Some(IDLE_SEGMENT),
        SegmentKind::Blank => None,
    }
}

/// `0xRRGGBB` as straight sRGB RGBA, opaque.
pub fn srgb(hex: u32) -> [f32; 4] {
    [
        ((hex >> 16) & 0xFF) as f32 / 255.0,
        ((hex >> 8) & 0xFF) as f32 / 255.0,
        (hex & 0xFF) as f32 / 255.0,
        1.0,
    ]
}

/// `0xRRGGBB` as linear RGBA with `alpha` — what the shape and text pipelines take.
pub fn linear(hex: u32, alpha: f32) -> [f32; 4] {
    crate::sidebar_theme::linear(hex, alpha)
}

/// Straight sRGB RGBA to linear RGBA (alpha untouched).
pub fn to_linear(c: [f32; 4]) -> [f32; 4] {
    fn ch(u: f32) -> f32 {
        if u <= 0.04045 {
            u / 12.92
        } else {
            ((u + 0.055) / 1.055).powf(2.4)
        }
    }
    [ch(c[0]), ch(c[1]), ch(c[2]), c[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_and_linear_agree_with_the_sidebar_conversion() {
        for hex in [BG, NEEDS_YOU_FILL, ACCENT, DONE, 0xFFFFFF, 0x000000] {
            let a = to_linear(srgb(hex));
            let b = linear(hex, 1.0);
            for k in 0..4 {
                assert!((a[k] - b[k]).abs() < 1e-5, "{hex:06X}: {a:?} vs {b:?}");
            }
        }
        assert_eq!(srgb(0x30271B)[0], 0x30 as f32 / 255.0);
        assert_eq!(srgb(0x30271B)[3], 1.0);
    }

    #[test]
    fn roles_map_to_the_design_colours() {
        assert_eq!(subagent(SubagentStatus::Running), 0xE8744F);
        assert_eq!(subagent(SubagentStatus::Done), 0x6FA67F);
        assert_eq!(subagent(SubagentStatus::Failed), 0xE5534B);
        assert_eq!(agent(AgentTag::Claude), 0xE9967A);
        assert_eq!(agent(AgentTag::Codex), 0xA9B8FF);
        assert_eq!(agent(AgentTag::Antigravity), 0x86D1C4);
        assert_eq!(agent(AgentTag::Gemini), 0xB8B1A7);
        assert_eq!(agent(AgentTag::Aider), 0xB8B1A7);
        assert_eq!(segment(SegmentKind::Blank), None);
        assert_eq!(segment(SegmentKind::Idle), Some(0x3A3632));
        assert_eq!(segment(SegmentKind::WaitingOnSubagent), Some(0x6B3A2A));
        assert_eq!(segment(SegmentKind::Working), Some(0xE8744F));
        assert_eq!(bucket_label(Bucket::Agent), "Agent");
        assert_eq!(pill(Bucket::NeedsYou).1, WAITING);
    }
}

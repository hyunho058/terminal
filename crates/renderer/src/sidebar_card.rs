//! The weather card, composed as data (sidebar-weather-cards D16).
//!
//! [`build_card`] turns one card's plain inputs into the shapes and positioned text runs
//! that draw it — skin gradient and state border, the selected glow, the pixel agent
//! icon, the hover `≡`, the title, the folder line, the status and the branch — and
//! touches no GPU state, so every decision the card makes is assertable in a unit test.
//! The row layer (`sidebar_row::TabBar`) only resolves the runs' glyphs through the card
//! atlases and submits.
//!
//! **Zones** (the Anatomy board): top-left the title over the folder line, top-right
//! the agent icon (and the `≡` on hover, left of it), bottom-left the status, bottom-
//! right the branch. Each zone truncates inside its own budget (D14), by display column
//! rather than by char (D3), so a Hangul title never overlaps itself.

use crate::sidebar_icons::{push_icon_rgba, push_pixel_icon, Icon, PixelIcon};
use crate::sidebar_shapes::{Fill, SidebarShape};
use crate::sidebar_text::CardFace;
use crate::sidebar_theme::{
    linear, AgentSkin, CardMetrics, SidebarTheme, CARD_BORDER_ALPHA, CARD_BORDER_HOVER_ALPHA,
    CARD_META_ALPHA, MENU_BACKPLATE_ALPHA,
};
use crate::tab_bar::char_cols;

/// What the card's bottom-left zone says (D7), or that it says nothing
/// ([`CardStatus::Hidden`]). Resolved by the app — the precedence agent > running >
/// failed > shell is policy, and lives there — and only drawn here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CardStatus {
    /// An agent is mid-turn. (No longer also an unknown state: agent-card-status D3
    /// reverses D7's `Unknown → Working`, and such a card is [`CardStatus::Hidden`].)
    Working,
    /// An agent is idle.
    Ready,
    /// An agent handed the turn back and waits for the user.
    WaitingForInput,
    /// No agent; the shell is running a foreground command (OSC 133 `C`, no `D` yet).
    Running,
    /// No agent, nothing running, and the last command failed with this code.
    Exited(i32),
    /// No agent, nothing running, nothing failed: the plain shell (`zsh`).
    #[default]
    Shell,
    /// The status is not known, so the card states none: no icon, no label, and the
    /// branch laid out exactly as with `[sidebar].show_agent_status` off (D13) — one
    /// definition of a card without a status (agent-card-status D9). Which tabs this is
    /// true of is the app's policy; this layer is told only "no status", never why.
    Hidden,
}

/// A card's interaction state (D4). Selection outranks hover — the caller resolves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardState {
    Default,
    Hover,
    Selected,
}

/// A card's state from the two facts the layer holds (D4): the active card is
/// `Selected` whether or not the pointer is over it — selection outranks hover (R4.4).
pub fn card_state(active: bool, hovered: bool) -> CardState {
    match (active, hovered) {
        (true, _) => CardState::Selected,
        (false, true) => CardState::Hover,
        (false, false) => CardState::Default,
    }
}

/// One card's inputs. Plain values only, so a test builds one by struct literal.
#[derive(Clone, Debug)]
pub struct CardInput<'a> {
    /// The card box, physical px `[x, y, w, h]`. The selected glow is drawn outside it.
    pub rect: [f32; 4],
    pub skin: AgentSkin,
    pub state: CardState,
    pub status: CardStatus,
    /// Line 1, already resolved by the app (user > AI task > program > cwd).
    pub title: &'a str,
    /// The folder line — `~`-abbreviated, or the pre-publish fallback (see
    /// [`folder_text`]).
    pub folder: &'a str,
    pub branch: Option<&'a str>,
    /// `[sidebar].show_git_branch` (D13).
    pub show_branch: bool,
    /// `[sidebar].show_agent_status` (D13).
    pub show_status: bool,
    /// Draw the `≡`: the card is hovered, its menu is open, or it is being renamed (D11).
    pub show_menu: bool,
    /// The card is being renamed: its title run is left out, because the layer draws the
    /// edit field over [`CardDraw::title_zone`] instead (D15).
    pub editing: bool,
    pub scale: f64,
    /// Column width and line height of each face, physical px — the card atlases' own
    /// (`CardFace::column_px` / `line_px`), or the grid cell's when a card atlas could
    /// not be built (R2.6).
    pub title_col: f32,
    pub title_line: f32,
    pub meta_col: f32,
    pub meta_line: f32,
    /// `…` when the font has it, else `.`.
    pub ellipsis: char,
}

/// A positioned string: drawn from `face`'s atlas, starting at `x` with its line box's
/// top at `top`, advancing one display column per column (D3).
#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    pub face: CardFace,
    pub text: String,
    pub x: f32,
    pub top: f32,
    pub color: [f32; 4],
    pub bold: bool,
}

/// Everything [`build_card`] composed for one card.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CardDraw {
    /// In submit order: glow, body, icons, backplate — later over earlier.
    pub shapes: Vec<SidebarShape>,
    pub runs: Vec<TextRun>,
    /// Where the title line sits and how far it may reach, physical px `[x, y, w, h]` —
    /// the box the rename field takes over (D15). Its right edge is D14's title budget.
    pub title_zone: [f32; 4],
}

/// The width reserved for the agent icon at `scale`, physical px: the widest icon **as
/// drawn**. Icons are right-aligned inside it.
///
/// It exists because the `≡` must sit at **one** position the hit-test can compute
/// without knowing the card's skin (SD2 of `specs/sidebar-redesign/partition-manifest.md`;
/// `SidebarLayoutInput` carries no per-card data). Anchoring the button to this slot
/// rather than to each icon's own left edge puts it the mock's 8px from the widest icon
/// and further left on the others — see the spec's D11 note.
///
/// Measured, not the crab's 43px design width: whole-pixel cells (D6) draw the crab at
/// 51px at 1× and about 45 at 1.5× and 3×, and a 43px slot left the button touching it.
pub fn icon_slot_w(scale: f64) -> f32 {
    PixelIcon::ALL
        .iter()
        .map(|icon| icon.extent(scale)[0])
        .fold(0.0, f32::max)
}

/// The `≡` backplate's rect inside `card` (physical px `[x, y, w, h]`) at `scale` —
/// skin-independent by construction, so `sidebar_layout::menu_button_in` can be this
/// same function and the drawn button is the hit-tested one (R11.4).
pub fn menu_button(card: [f32; 4], scale: f64) -> [f32; 4] {
    let m = CardMetrics::DEFAULT.scaled(scale);
    let inner_right = card[0] + card[2] - m.border - m.pad_x;
    let inner_top = card[1] + m.border + m.pad_y;
    let x = inner_right - icon_slot_w(scale) - m.menu_gap - m.menu_button;
    [x, inner_top + m.menu_top, m.menu_button, m.menu_button]
}

/// The folder line's text: the live `~`-abbreviated path the app pulled, or — before the
/// first metadata fill, when that is empty — the row's assembly-time folder name
/// (`specs/card-live-cwd/spec.md` D2; R10.4).
pub fn folder_text<'a>(cwd_display: &'a str, row_title: &'a str) -> &'a str {
    if cwd_display.is_empty() {
        row_title
    } else {
        cwd_display
    }
}

/// Display width of `s` in columns (D3).
fn cols(s: &str) -> usize {
    s.chars().map(char_cols).sum()
}

/// The prefix of `s` that fits `max_cols` columns, with `ellipsis` appended when anything
/// was cut — counting the ellipsis's own column. A wide char is taken whole or not at
/// all, so nothing is half-drawn and no codepoint is split (D3/R3.2).
pub fn truncate_cols_right(s: &str, max_cols: usize, ellipsis: char) -> String {
    if cols(s) <= max_cols {
        return s.to_string();
    }
    let e = char_cols(ellipsis);
    if max_cols < e {
        return String::new();
    }
    // The same column walk as the tab bar's, leaving the ellipsis its column.
    let mut out = crate::tab_bar::truncate_cols(s, max_cols - e);
    out.push(ellipsis);
    out
}

/// The suffix of `s` that fits `max_cols` columns, with `ellipsis` **prepended** when
/// anything was cut: the folder line keeps the tail — the part that names the folder —
/// and loses the head (`…/IdeaProjects/terminal`, D10/R10.3).
pub fn truncate_cols_left(s: &str, max_cols: usize, ellipsis: char) -> String {
    if cols(s) <= max_cols {
        return s.to_string();
    }
    let e = char_cols(ellipsis);
    if max_cols < e {
        return String::new();
    }
    let mut used = 0;
    let mut tail: Vec<char> = Vec::new();
    for c in s.chars().rev() {
        let w = char_cols(c);
        if used + w > max_cols - e {
            break;
        }
        used += w;
        tail.push(c);
    }
    let mut out = String::with_capacity(tail.len() + 1);
    out.push(ellipsis);
    out.extend(tail.iter().rev());
    out
}

/// How many whole columns of width `col` fit in `px` — none for a non-positive budget
/// or a degenerate column (which would otherwise divide to infinity).
fn fit_cols(px: f32, col: f32) -> usize {
    if col <= 0.0 {
        return 0;
    }
    (px / col).floor().max(0.0) as usize
}

/// How the status zone draws one [`CardStatus`] (R7.6).
struct StatusLook {
    label: String,
    bold: bool,
    color: [f32; 4],
    icon: Icon,
    /// The icon's size, physical px — D21's size through [`CardMetrics::scaled`].
    icon_px: f32,
}

/// The status zone's label, weight, colour and icon (R7.6) — `None` for
/// [`CardStatus::Hidden`], which draws no zone at all (agent-card-status D9).
fn status_look(
    status: CardStatus,
    skin: AgentSkin,
    theme: &SidebarTheme,
    m: &CardMetrics,
) -> Option<StatusLook> {
    let look = |label: String, bold: bool, color: [f32; 4], icon: Icon, icon_px: f32| {
        Some(StatusLook {
            label,
            bold,
            color,
            icon,
            icon_px,
        })
    };
    let meta = linear(theme.card_meta_fg, CARD_META_ALPHA);
    match status {
        CardStatus::Working => {
            let fg = if skin == AgentSkin::Codex {
                theme.status_working_codex_fg
            } else {
                theme.status_working_fg
            };
            look(
                "Working".into(),
                false,
                linear(fg, 1.0),
                Icon::CardAsterisk,
                m.zone_icon_lg,
            )
        }
        CardStatus::Ready => look(
            "Ready".into(),
            false,
            linear(theme.status_ready_fg, 1.0),
            Icon::CardDiamond,
            m.zone_icon,
        ),
        CardStatus::WaitingForInput => look(
            "Waiting for input".into(),
            true,
            linear(theme.status_waiting_fg, 1.0),
            Icon::CardPause,
            m.zone_icon,
        ),
        CardStatus::Exited(code) => look(
            format!("Exited {code}"),
            true,
            linear(theme.status_error_fg, 1.0),
            Icon::CardCross,
            m.zone_icon,
        ),
        CardStatus::Running => look("Running".into(), false, meta, Icon::Prompt, m.zone_icon_lg),
        CardStatus::Shell => look("zsh".into(), false, meta, Icon::Prompt, m.zone_icon_lg),
        CardStatus::Hidden => None,
    }
}

/// Compose one card (D16). Pure: same input, same output, no GPU.
pub fn build_card(inp: &CardInput<'_>) -> CardDraw {
    let theme = SidebarTheme::DEFAULT;
    let m = CardMetrics::DEFAULT.scaled(inp.scale);
    let [x, y, w, h] = inp.rect;
    let skin = inp.skin.card();
    let mut out = CardDraw::default();

    // ---- Body (D4): glow first so the body paints over all but its outer ring.
    if inp.state == CardState::Selected {
        let g = m.glow;
        if let Some(glow) = SidebarShape::rounded_box(
            [x - g, y - g, w + 2.0 * g, h + 2.0 * g],
            m.radius + g,
            0.0,
            Fill::solid(skin.glow),
            [0.0; 4],
        ) {
            out.shapes.push(glow);
        }
    }
    let white = |a: f32| linear(0x00FF_FFFF, a);
    let (fill, border_w, border) = match inp.state {
        CardState::Default => (skin.default, m.border, white(CARD_BORDER_ALPHA)),
        CardState::Hover => (skin.hover, m.border, white(CARD_BORDER_HOVER_ALPHA)),
        CardState::Selected => (skin.hover, m.border_selected, skin.tint),
    };
    if let Some(body) = SidebarShape::rounded_box(
        inp.rect,
        m.radius,
        border_w,
        Fill::gradient(fill.stops, fill.mid, fill.dir),
        border,
    ) {
        out.shapes.push(body);
    }

    // The padding box. Measured from a constant 1px border, not the state's, so nothing
    // inside the card moves by half a pixel when it is selected.
    let left = x + m.border + m.pad_x;
    let right = x + w - m.border - m.pad_x;
    let top = y + m.border + m.pad_y;
    let bottom = y + h - m.border - m.pad_y;

    // ---- Top-right: the agent icon, right-aligned in its slot (D6).
    let icon = PixelIcon::for_skin(inp.skin);
    let [iw, _] = icon.extent(inp.scale);
    let icon_top = if icon == PixelIcon::Claude {
        m.icon_top_claude
    } else {
        m.icon_top_other
    };
    push_pixel_icon(&mut out.shapes, icon, right - iw, top + icon_top, inp.scale);

    // ---- The `≡` (D11), left of the icon slot, on the one skin-independent rect.
    let title_right = if inp.show_menu {
        let b = menu_button([x, y, w, h], inp.scale);
        if let Some(plate) = SidebarShape::rounded_box(
            b,
            m.menu_radius,
            0.0,
            Fill::solid(white(MENU_BACKPLATE_ALPHA)),
            [0.0; 4],
        ) {
            out.shapes.push(plate);
        }
        let inset = (m.menu_button - m.menu_glyph) * 0.5;
        push_icon_rgba(
            &mut out.shapes,
            Icon::Hamburger,
            b[0] + inset,
            b[1] + inset,
            m.menu_glyph,
            linear(theme.card_title_fg, 1.0),
        );
        b[0] - m.zone_gap
    } else {
        right - iw - m.zone_gap
    };

    // ---- Top-left: the title (D14 budget) …
    let title_w = (title_right - left).max(0.0);
    out.title_zone = [left, top, title_w, inp.title_line];
    if !inp.editing {
        out.runs.push(TextRun {
            face: CardFace::Title,
            text: truncate_cols_right(inp.title, fit_cols(title_w, inp.title_col), inp.ellipsis),
            x: left,
            top,
            color: linear(theme.card_title_fg, 1.0),
            bold: true,
        });
    }

    // … over the folder line: icon, gap, then the path truncated from the left (D10).
    let meta_fg = linear(theme.card_meta_fg, CARD_META_ALPHA);
    let folder_top = top + inp.title_line + m.title_gap;
    let folder_icon = m.zone_icon_lg;
    push_icon_rgba(
        &mut out.shapes,
        Icon::Folder,
        left,
        folder_top + (inp.meta_line - folder_icon) * 0.5,
        folder_icon,
        meta_fg,
    );
    let folder_x = left + folder_icon + m.icon_text_gap;
    out.runs.push(TextRun {
        face: CardFace::Meta,
        text: truncate_cols_left(
            inp.folder,
            fit_cols(title_right - folder_x, inp.meta_col),
            inp.ellipsis,
        ),
        x: folder_x,
        top: folder_top,
        color: meta_fg,
        bold: false,
    });

    // ---- Bottom row: the status first, at its natural width (D14) …
    //
    // The toggle off (D13) and a `Hidden` status both come out of this one `Option` as
    // `None`, and nothing below reads `show_status` or `status` again — so there is one
    // definition of a card without a status, not two that could drift (agent-card-status
    // D9). Its branch floor stays at the padding box's left.
    let row_top = bottom - inp.meta_line;
    let mut branch_floor = left;
    let look = if inp.show_status {
        status_look(inp.status, inp.skin, &theme, &m)
    } else {
        None
    };
    if let Some(look) = look {
        let size = look.icon_px;
        push_icon_rgba(
            &mut out.shapes,
            look.icon,
            left,
            row_top + (inp.meta_line - size) * 0.5,
            size,
            look.color,
        );
        let tx = left + size + m.icon_text_gap;
        let status_end = tx + cols(&look.label) as f32 * inp.meta_col;
        branch_floor = status_end + m.zone_gap;
        out.runs.push(TextRun {
            face: CardFace::Meta,
            text: look.label,
            x: tx,
            top: row_top,
            color: look.color,
            bold: look.bold,
        });
    }

    // … then the branch, right-aligned into what is left, or dropped when not even its
    // icon fits (D14).
    if let (true, Some(branch)) = (inp.show_branch, inp.branch) {
        let icon_px = m.zone_icon;
        let lead = icon_px + m.branch_icon_gap;
        let room = right - branch_floor;
        if room >= icon_px {
            let text =
                truncate_cols_right(branch, fit_cols(room - lead, inp.meta_col), inp.ellipsis);
            let text_w = cols(&text) as f32 * inp.meta_col;
            // No room for even one column: the icon alone, still right-aligned.
            let bx = if text.is_empty() {
                right - icon_px
            } else {
                right - lead - text_w
            };
            push_icon_rgba(
                &mut out.shapes,
                Icon::CardBranch,
                bx,
                row_top + (inp.meta_line - icon_px) * 0.5,
                icon_px,
                meta_fg,
            );
            if !text.is_empty() {
                out.runs.push(TextRun {
                    face: CardFace::Meta,
                    text,
                    x: bx + lead,
                    top: row_top,
                    color: meta_fg,
                    bold: false,
                });
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar_shapes::KIND_ROUNDED_BOX;
    use crate::sidebar_text::{column_px, line_px};

    /// A 340-logical sidebar at 2×: the card box is inset 14 → x 28, w 680 − 56.
    const RECT: [f32; 4] = [28.0, 100.0, 624.0, 208.0];
    /// The padding box of `RECT`: 1px border + 16/12 padding, at 2×.
    const LEFT: f32 = 62.0;
    const RIGHT: f32 = 618.0;
    const TOP: f32 = 126.0;
    const BOTTOM: f32 = 282.0;

    fn input(skin: AgentSkin, state: CardState) -> CardInput<'static> {
        CardInput {
            rect: RECT,
            skin,
            state,
            status: CardStatus::Working,
            title: "refactor",
            folder: "~/IdeaProjects/terminal",
            branch: Some("feat/cards"),
            show_branch: true,
            show_status: true,
            show_menu: false,
            editing: false,
            scale: 2.0,
            title_col: column_px(34.0),
            title_line: line_px(34.0),
            meta_col: column_px(25.0),
            meta_line: line_px(25.0),
            ellipsis: '…',
        }
    }

    fn body(d: &CardDraw) -> &SidebarShape {
        d.shapes
            .iter()
            .find(|s| s.bbox == RECT)
            .expect("a body shape")
    }

    fn run<'d>(d: &'d CardDraw, text: &str) -> Option<&'d TextRun> {
        d.runs.iter().find(|r| r.text == text)
    }

    fn run_right(r: &TextRun, col: f32) -> f32 {
        r.x + cols(&r.text) as f32 * col
    }

    fn white(a: f32) -> [f32; 4] {
        linear(0x00FF_FFFF, a)
    }

    // ---- D3: columns ------------------------------------------------------------

    #[test]
    fn wide_characters_advance_two_columns() {
        // R3.1 — `사이드바 카드`: four wide syllables, a space, two more.
        let pens: Vec<f32> = crate::tab_bar::pen_layout("사이드바 카드", 0.0, 10.0)
            .map(|(_, x)| x)
            .collect();
        assert_eq!(pens, vec![0.0, 20.0, 40.0, 60.0, 80.0, 90.0, 110.0]);
    }

    #[test]
    fn truncation_counts_columns_and_keeps_wide_glyphs_whole() {
        // R3.2 — nine columns including the ellipsis: `auth 마` is 7, `이` would make 9
        // before the ellipsis, so it stops at `auth 마…` (8).
        assert_eq!(truncate_cols_right("auth 마이그레이션", 9, '…'), "auth 마…");
        assert_eq!(
            truncate_cols_right("short", 9, '…'),
            "short",
            "fits → untouched"
        );
        assert_eq!(truncate_cols_right("abcdef", 6, '…'), "abcdef", "exact fit");
        assert_eq!(truncate_cols_right("abcdefg", 6, '…'), "abcde…");
        assert_eq!(truncate_cols_right("abc", 0, '…'), "", "no room at all");
        assert_eq!(
            truncate_cols_right("abc", 1, '…'),
            "…",
            "room for the mark only"
        );
        assert!(cols(&truncate_cols_right("한글한글", 3, '…')) <= 3);
    }

    #[test]
    fn a_long_path_keeps_its_tail() {
        // R10.3 — `…` then the last 19 columns, 20 in all.
        let p = "~/IdeaProjects/terminal-feat-improve-card-ui";
        let t = truncate_cols_left(p, 20, '…');
        assert_eq!(t, format!("…{}", &p[p.len() - 19..]));
        assert_eq!(cols(&t), 20);
        assert_eq!(
            truncate_cols_left("~/a", 20, '…'),
            "~/a",
            "fits → untouched"
        );
        // Wide chars whole: the tail `글` (2) fits after the mark, `한글` (4) would not.
        assert_eq!(
            truncate_cols_left("~/한글", 4, '…'),
            "…글",
            "wide chars whole"
        );
        assert_eq!(truncate_cols_left("abc", 0, '…'), "");
        assert_eq!(
            truncate_cols_left("abc", 1, '…'),
            "…",
            "room for the mark only"
        );
    }

    #[test]
    fn the_folder_falls_back_to_the_row_title_before_the_first_fill() {
        // R10.4.
        assert_eq!(folder_text("", "terminal"), "terminal");
        assert_eq!(folder_text("~/x", "terminal"), "~/x", "live wins");
    }

    // ---- D4: states ---------------------------------------------------------------

    #[test]
    fn the_default_state() {
        // R4.1 — default gradient, 2px white .10 border, radius 32, no glow.
        let d = build_card(&input(AgentSkin::Claude, CardState::Default));
        let b = body(&d);
        let skin = AgentSkin::Claude.card();
        assert_eq!(b.kind, KIND_ROUNDED_BOX);
        assert_eq!(b.geom, [32.0, 2.0], "radius 16, border 1, at 2×");
        assert_eq!(b.fill0, skin.default.stops[0]);
        assert_eq!(b.fill2, skin.default.stops[2]);
        assert_eq!(b.border_color, white(CARD_BORDER_ALPHA));
        assert!(!d.shapes.iter().any(|s| s.bbox[0] < RECT[0]), "no glow");
    }

    #[test]
    fn the_hover_state() {
        // R4.2.
        let d = build_card(&input(AgentSkin::Claude, CardState::Hover));
        let b = body(&d);
        assert_eq!(b.geom, [32.0, 2.0]);
        assert_eq!(b.fill0, AgentSkin::Claude.card().hover.stops[0]);
        assert_eq!(b.border_color, white(CARD_BORDER_HOVER_ALPHA));
        assert!(!d.shapes.iter().any(|s| s.bbox[0] < RECT[0]), "no glow");
    }

    #[test]
    fn the_selected_state_draws_a_glow_behind_the_card() {
        // R4.3 — the glow is emitted first: the rect grown 6, radius 38, glow colour;
        // then the body with the hover gradient and a 3px tint border.
        let d = build_card(&input(AgentSkin::Claude, CardState::Selected));
        let skin = AgentSkin::Claude.card();
        let glow = &d.shapes[0];
        assert_eq!(glow.bbox, [22.0, 94.0, 636.0, 220.0]);
        assert_eq!(glow.geom, [38.0, 0.0]);
        assert_eq!(glow.fill0, skin.glow);
        assert_eq!(d.shapes[1].bbox, RECT, "the body right after it");
        let b = body(&d);
        assert_eq!(b.geom, [32.0, 3.0], "a 1.5px border at 2×");
        assert_eq!(b.border_color, skin.tint);
        assert_eq!(b.fill0, skin.hover.stops[0]);
    }

    #[test]
    fn selection_outranks_hover() {
        // R4.4.
        assert_eq!(card_state(true, true), CardState::Selected);
        assert_eq!(card_state(true, false), CardState::Selected);
        assert_eq!(card_state(false, true), CardState::Hover);
        assert_eq!(card_state(false, false), CardState::Default);
    }

    #[test]
    fn a_plain_idle_card_has_a_body() {
        // R4.5 — reverses sidebar-row-redesign D12's "no instance".
        let d = build_card(&input(AgentSkin::Generic, CardState::Default));
        assert_eq!(body(&d).fill0, AgentSkin::Generic.card().default.stops[0]);
    }

    // ---- D6: the agent icon ---------------------------------------------------------

    /// The zero-radius, borderless cells — the pixel icon.
    fn cells(d: &CardDraw) -> Vec<&SidebarShape> {
        d.shapes
            .iter()
            .filter(|s| s.geom == [0.0, 0.0] && s.kind == KIND_ROUNDED_BOX && s.bbox[1] < 200.0)
            .collect()
    }

    #[test]
    fn the_icon_follows_the_skin() {
        // R6.1 — by a colour only that icon has; a Gemini tab is a Generic skin and
        // gets the terminal.
        for (skin, rgb, n) in [
            (AgentSkin::Claude, 0x00E6_714E, 10),
            (AgentSkin::Antigravity, 0x00FF_8C00, 50),
            (AgentSkin::Codex, 0x001C_2A27, 9),
            (AgentSkin::Generic, 0x0015_130F, 11),
        ] {
            let d = build_card(&input(skin, CardState::Default));
            let c = cells(&d);
            assert_eq!(c.len(), n, "{skin:?}");
            assert!(c.iter().any(|s| s.fill0 == linear(rgb, 1.0)), "{skin:?}");
        }
    }

    #[test]
    fn the_icon_sits_top_right() {
        // R6.5 — right edge on the padding box's right; top 8 (Claude) or 4 below it.
        for (skin, top) in [(AgentSkin::Claude, 8.0), (AgentSkin::Codex, 4.0)] {
            let d = build_card(&input(skin, CardState::Default));
            let c = cells(&d);
            let right = c
                .iter()
                .map(|s| s.bbox[0] + s.bbox[2])
                .fold(f32::MIN, f32::max);
            let min_y = c.iter().map(|s| s.bbox[1]).fold(f32::MAX, f32::min);
            assert_eq!(right, RIGHT, "{skin:?} right edge");
            assert_eq!(min_y, TOP + top, "{skin:?} top");
        }
    }

    // ---- D7 / D9 / D21: the status zone ---------------------------------------------

    #[test]
    fn each_status_draws_its_label_weight_colour_and_icon() {
        // R7.6 — the table, on a non-Codex card.
        let t = SidebarTheme::DEFAULT;
        let meta = linear(t.card_meta_fg, CARD_META_ALPHA);
        for (status, text, bold, color) in [
            (
                CardStatus::Working,
                "Working",
                false,
                linear(t.status_working_fg, 1.0),
            ),
            (
                CardStatus::Ready,
                "Ready",
                false,
                linear(t.status_ready_fg, 1.0),
            ),
            (
                CardStatus::WaitingForInput,
                "Waiting for input",
                true,
                linear(t.status_waiting_fg, 1.0),
            ),
            (
                CardStatus::Exited(101),
                "Exited 101",
                true,
                linear(t.status_error_fg, 1.0),
            ),
            (
                CardStatus::Exited(-1),
                "Exited -1",
                true,
                linear(t.status_error_fg, 1.0),
            ),
            (CardStatus::Running, "Running", false, meta),
            (CardStatus::Shell, "zsh", false, meta),
        ] {
            let mut inp = input(AgentSkin::Claude, CardState::Default);
            inp.status = status;
            let d = build_card(&inp);
            let r = run(&d, text).unwrap_or_else(|| panic!("{status:?} label"));
            assert_eq!(
                (r.bold, r.color, r.face),
                (bold, color, CardFace::Meta),
                "{status:?}"
            );
            assert_eq!(
                r.top,
                BOTTOM - line_px(25.0),
                "{status:?} on the bottom row"
            );
            // Its icon: shapes in the status colour, left of the label, on its row.
            assert!(
                d.shapes.iter().any(|s| s.fill0 == color
                    && s.bbox[0] >= LEFT - 4.0
                    && s.bbox[0] + s.bbox[2] <= r.x + 1.0
                    && s.bbox[1] >= r.top - 2.0),
                "{status:?} icon"
            );
        }
    }

    #[test]
    fn the_status_icons_are_the_right_ones() {
        // The Waiting icon is the only one made of two filled bars; the X the only one
        // with 3.0-unit strokes.
        let mut inp = input(AgentSkin::Claude, CardState::Default);
        inp.status = CardStatus::WaitingForInput;
        let waiting = linear(SidebarTheme::DEFAULT.status_waiting_fg, 1.0);
        let bars = build_card(&inp)
            .shapes
            .iter()
            .filter(|s| s.fill0 == waiting && s.kind == KIND_ROUNDED_BOX)
            .count();
        assert_eq!(bars, 2, "two filled bars");
        inp.status = CardStatus::Exited(2);
        let error = linear(SidebarTheme::DEFAULT.status_error_fg, 1.0);
        let d = build_card(&inp);
        let x: Vec<_> = d.shapes.iter().filter(|s| s.fill0 == error).collect();
        assert_eq!(x.len(), 2, "an X is two strokes");
        // D21: the cross's 24-unit stroke, at its 12-logical size drawn at 2×.
        let size = 12.0 * 2.0;
        let want = crate::sidebar_icons::stroke_w(Icon::CardCross) * size / 24.0;
        assert!(x.iter().all(|s| (s.geom[0] * 2.0 - want).abs() < 1e-4));
    }

    #[test]
    fn working_is_tinted_per_skin() {
        // R9.3.
        let t = SidebarTheme::DEFAULT;
        for (skin, fg) in [
            (AgentSkin::Codex, t.status_working_codex_fg),
            (AgentSkin::Claude, t.status_working_fg),
            (AgentSkin::Antigravity, t.status_working_fg),
            (AgentSkin::Generic, t.status_working_fg),
        ] {
            let d = build_card(&input(skin, CardState::Default));
            assert_eq!(
                run(&d, "Working").expect("label").color,
                linear(fg, 1.0),
                "{skin:?}"
            );
        }
    }

    #[test]
    fn every_icon_takes_its_texts_colour() {
        // R9.4 — the folder and branch icons carry the secondary text's .78, the status
        // icon its status colour: each shape left of a run on that run's row is that
        // run's colour.
        let d = build_card(&input(AgentSkin::Claude, CardState::Default));
        for text in ["~/IdeaProjects/terminal", "feat/cards", "Working"] {
            let r = run(&d, text).expect(text);
            let lead: Vec<_> = d
                .shapes
                .iter()
                .filter(|s| {
                    s.bbox[1] >= r.top - 2.0
                        && s.bbox[1] + s.bbox[3] <= r.top + line_px(25.0) + 2.0
                        && s.bbox[0] + s.bbox[2] <= r.x + 1.0
                        && s.bbox[0] + s.bbox[2] >= r.x - 40.0
                })
                .collect();
            assert!(!lead.is_empty(), "{text} has an icon");
            assert!(
                lead.iter().all(|s| s.fill0 == r.color),
                "{text} icon colour"
            );
        }
    }

    /// True when the card contains exactly the shapes `icon` emits at `(x, y, size)` in
    /// `col` — pins the icon's kind, size, position and colour in one comparison.
    fn has_icon(d: &CardDraw, icon: Icon, x: f32, y: f32, size: f32, col: [f32; 4]) -> bool {
        let mut want = Vec::new();
        push_icon_rgba(&mut want, icon, x, y, size, col);
        !want.is_empty() && want.iter().all(|w| d.shapes.contains(w))
    }

    #[test]
    fn zone_icons_are_centred_on_their_lines_at_their_sizes() {
        // D21 — each icon is its table size (×2), vertically centred on its text line,
        // and its text starts one gap after it (6 logical; the branch's 5).
        let t = SidebarTheme::DEFAULT;
        let meta = linear(t.card_meta_fg, CARD_META_ALPHA);
        let (ml, tl) = (line_px(25.0), line_px(34.0));
        let d = build_card(&input(AgentSkin::Claude, CardState::Default));
        let folder_top = TOP + tl + 6.0;
        assert!(has_icon(
            &d,
            Icon::Folder,
            LEFT,
            folder_top + (ml - 26.0) * 0.5,
            26.0,
            meta
        ));
        let row = BOTTOM - ml;
        let working = linear(t.status_working_fg, 1.0);
        assert!(has_icon(
            &d,
            Icon::CardAsterisk,
            LEFT,
            row + (ml - 26.0) * 0.5,
            26.0,
            working
        ));
        assert_eq!(run(&d, "Working").expect("status").x, LEFT + 26.0 + 12.0);
        let b = run(&d, "feat/cards").expect("branch");
        let bx = b.x - 24.0 - 10.0;
        assert!(has_icon(
            &d,
            Icon::CardBranch,
            bx,
            row + (ml - 24.0) * 0.5,
            24.0,
            meta
        ));
        // A 12-logical status icon too.
        let mut inp = input(AgentSkin::Claude, CardState::Default);
        inp.status = CardStatus::Ready;
        let d = build_card(&inp);
        let ready = linear(t.status_ready_fg, 1.0);
        assert!(has_icon(
            &d,
            Icon::CardDiamond,
            LEFT,
            row + (ml - 24.0) * 0.5,
            24.0,
            ready
        ));
        assert_eq!(run(&d, "Ready").expect("status").x, LEFT + 24.0 + 12.0);
    }

    #[test]
    fn the_hamburger_is_centred_in_its_plate() {
        // D11 — a 28px glyph inset 12 on each side of the 52px plate.
        let mut inp = input(AgentSkin::Claude, CardState::Hover);
        inp.show_menu = true;
        let d = build_card(&inp);
        let b = menu_button(RECT, 2.0);
        let glyph = linear(SidebarTheme::DEFAULT.card_title_fg, 1.0);
        assert!(has_icon(
            &d,
            Icon::Hamburger,
            b[0] + 12.0,
            b[1] + 12.0,
            28.0,
            glyph
        ));
    }

    #[test]
    fn fit_cols_never_divides_by_a_degenerate_column() {
        assert_eq!(fit_cols(100.0, 15.0), 6);
        assert_eq!(fit_cols(90.0, 15.0), 6, "an exact fit counts");
        assert_eq!(fit_cols(10.0, 0.0), 0, "a zero column fits nothing");
        assert_eq!(fit_cols(10.0, -1.0), 0);
        assert_eq!(fit_cols(-5.0, 15.0), 0, "a negative budget fits nothing");
    }

    // ---- D11: the `≡` -----------------------------------------------------------------

    fn backplate(d: &CardDraw) -> Option<&SidebarShape> {
        d.shapes.iter().find(|s| s.bbox == menu_button(RECT, 2.0))
    }

    #[test]
    fn no_button_at_rest() {
        // R11.1.
        let d = build_card(&input(AgentSkin::Claude, CardState::Default));
        assert!(backplate(&d).is_none());
        let glyph = linear(SidebarTheme::DEFAULT.card_title_fg, 1.0);
        assert!(
            !d.shapes.iter().any(|s| s.fill0 == glyph),
            "no hamburger strokes either"
        );
    }

    #[test]
    fn the_button_sits_left_of_the_icon_slot() {
        // R11.2 — 52×52, radius 16, white .16; right edge 16 left of the 85-wide slot
        // (the crab as drawn at 2×, D11), top 6 below the padding box; the glyph inside.
        let mut inp = input(AgentSkin::Codex, CardState::Hover);
        inp.show_menu = true;
        let d = build_card(&inp);
        let b = backplate(&d).expect("backplate");
        assert_eq!(b.bbox, [RIGHT - 85.0 - 16.0 - 52.0, TOP + 6.0, 52.0, 52.0]);
        assert_eq!(b.geom, [16.0, 0.0]);
        assert_eq!(b.fill0, white(MENU_BACKPLATE_ALPHA));
        // Counted inside the plate: the Codex bubble's white cells share the glyph's
        // `#f4efe6`, so a colour match alone would count them too.
        let glyph = linear(SidebarTheme::DEFAULT.card_title_fg, 1.0);
        let inside = |s: &&SidebarShape| {
            s.bbox[0] >= b.bbox[0]
                && s.bbox[0] + s.bbox[2] <= b.bbox[0] + b.bbox[2]
                && s.bbox[1] >= b.bbox[1]
                && s.bbox[1] + s.bbox[3] <= b.bbox[1] + b.bbox[3]
        };
        let strokes: Vec<_> = d
            .shapes
            .iter()
            .filter(|s| s.fill0 == glyph && s.bbox != b.bbox)
            .filter(inside)
            .collect();
        assert_eq!(strokes.len(), 3, "three rules");
        for s in strokes {
            assert!(s.bbox[0] >= b.bbox[0] && s.bbox[0] + s.bbox[2] <= b.bbox[0] + b.bbox[2]);
            assert!(s.bbox[1] >= b.bbox[1] && s.bbox[1] + s.bbox[3] <= b.bbox[1] + b.bbox[3]);
        }
    }

    #[test]
    fn the_button_does_not_depend_on_the_skin() {
        // D11's implementation note: one rect for every skin, so the hit-test needs none.
        let mut rects = Vec::new();
        for skin in [
            AgentSkin::Claude,
            AgentSkin::Antigravity,
            AgentSkin::Codex,
            AgentSkin::Generic,
        ] {
            let mut inp = input(skin, CardState::Hover);
            inp.show_menu = true;
            rects.push(backplate(&build_card(&inp)).expect("plate").bbox);
        }
        assert!(rects.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn the_button_keeps_the_mocks_gap_from_the_crab_at_every_scale() {
        // R11.5 / D11 (amended): the slot is the widest icon as drawn, so whole-pixel
        // cells (D6) cannot eat the gap — with the crab's 43px design width as the slot,
        // the gap was 0px at 1× and 5.7 logical at 1.5× and 3×. Within the half pixel
        // the crab's snapped origin may move.
        let crab = linear(0x00E6_714E, 1.0);
        for scale in [1.0, 1.25, 1.5, 2.0, 2.5, 3.0] {
            let s = scale as f32;
            let mut inp = input(AgentSkin::Claude, CardState::Hover);
            inp.rect = [14.0 * s, 50.0 * s, 312.0 * s, 104.0 * s];
            inp.scale = scale;
            inp.show_menu = true;
            let d = build_card(&inp);
            let plate = menu_button(inp.rect, scale);
            assert!(
                d.shapes.iter().any(|sh| sh.bbox == plate),
                "{scale}×: drawn at the hit-tested rect"
            );
            let crab_left = d
                .shapes
                .iter()
                .filter(|sh| sh.fill0 == crab)
                .map(|sh| sh.bbox[0])
                .fold(f32::MAX, f32::min);
            let gap = crab_left - (plate[0] + plate[2]);
            assert!(
                (gap - 8.0 * s).abs() <= 0.5,
                "{scale}×: {gap}px from the crab, want {}",
                8.0 * s
            );
        }
    }

    #[test]
    fn the_slot_is_the_widest_icon_as_drawn() {
        // The crab at every scale: 17 cells to the others' 12, and never rounded below
        // them. At 1× that is 51px, not the 43 of its design width.
        assert_eq!(icon_slot_w(1.0), 51.0);
        assert_eq!(icon_slot_w(2.0), 85.0);
        for scale in [1.0, 1.5, 2.0, 3.0] {
            for icon in PixelIcon::ALL {
                assert!(
                    icon.extent(scale)[0] <= icon_slot_w(scale),
                    "{icon:?} at {scale}×"
                );
            }
        }
    }

    // ---- D13: toggles -------------------------------------------------------------------

    #[test]
    fn the_branch_toggle_hides_the_branch() {
        // R12.1.
        let mut inp = input(AgentSkin::Claude, CardState::Default);
        let with = build_card(&inp);
        inp.show_branch = false;
        let without = build_card(&inp);
        assert!(run(&with, "feat/cards").is_some());
        assert!(run(&without, "feat/cards").is_none());
        assert!(without.shapes.len() < with.shapes.len(), "and its icon");
        assert_eq!(body(&without).bbox[3], 208.0, "height unchanged");
    }

    #[test]
    fn the_status_toggle_hides_the_status_but_not_the_identity() {
        // R12.2.
        let mut inp = input(AgentSkin::Claude, CardState::Default);
        inp.show_status = false;
        let d = build_card(&inp);
        assert!(run(&d, "Working").is_none());
        let working = linear(SidebarTheme::DEFAULT.status_working_fg, 1.0);
        assert!(
            !d.shapes.iter().any(|s| s.fill0 == working),
            "no status icon"
        );
        assert_eq!(cells(&d).len(), 10, "the crab stays");
        assert_eq!(
            body(&d).fill0,
            AgentSkin::Claude.card().default.stops[0],
            "skin stays"
        );
        assert!(run(&d, "feat/cards").is_some(), "the branch still shows");
    }

    // ---- agent-card-status D9: the no-status card ----------------------------------------

    /// [`input`] at any `scale`: the 312×104-logical card and the faces' 17 / 12.5 px,
    /// each through the atlas's own rounding — equal to `input` at 2×.
    fn input_at(scale: f64) -> CardInput<'static> {
        let s = scale as f32;
        let mut inp = input(AgentSkin::Claude, CardState::Default);
        inp.rect = [14.0 * s, 50.0 * s, 312.0 * s, 104.0 * s];
        inp.scale = scale;
        inp.title_col = column_px(17.0 * s);
        inp.title_line = line_px(17.0 * s);
        inp.meta_col = column_px(12.5 * s);
        inp.meta_line = line_px(12.5 * s);
        // Longer than the whole bottom row, so it is cut to whatever room the status
        // leaves: a `Hidden` card that kept any status floor would cut it shorter.
        inp.branch = Some("feat/a-branch-name-long-enough-to-need-the-whole-bottom-row-and-more");
        inp
    }

    #[test]
    fn a_hidden_status_lays_out_as_the_toggle_off_card() {
        // R5.4 — `Hidden` with the toggle on is byte-for-byte the toggle-off card, which
        // is the same card whatever its status says; and on the bottom row that card holds
        // the branch and nothing else.
        let drawable = [
            CardStatus::Working,
            CardStatus::Ready,
            CardStatus::WaitingForInput,
            CardStatus::Running,
            CardStatus::Exited(1),
            CardStatus::Shell,
            CardStatus::Hidden,
        ];
        for scale in [1.0, 2.0] {
            let s = scale as f32;
            let mut inp = input_at(scale);
            inp.status = CardStatus::Hidden;
            let hidden = build_card(&inp);
            for status in drawable {
                let mut off = input_at(scale);
                off.status = status;
                off.show_status = false;
                assert_eq!(build_card(&off), hidden, "{scale}×: {status:?} toggled off");
            }

            // No status run and no status icon: the bottom row is the branch alone —
            // its run, and exactly the 12-logical icon D21 puts 5 logical before it.
            let row = inp.rect[1] + inp.rect[3] - 13.0 * s - inp.meta_line;
            let runs: Vec<_> = hidden.runs.iter().filter(|r| r.top >= row).collect();
            assert_eq!(runs.len(), 1, "{scale}×: {runs:?}");
            let branch = runs[0];
            assert!(branch.text.starts_with("feat/a-"), "{scale}×: {branch:?}");
            assert!(branch.text.ends_with('…'), "{scale}×: cut to the room");
            let meta = linear(SidebarTheme::DEFAULT.card_meta_fg, CARD_META_ALPHA);
            let (icon, lead) = (12.0 * s, 17.0 * s);
            let mut want = Vec::new();
            push_icon_rgba(
                &mut want,
                Icon::CardBranch,
                branch.x - lead,
                row + (inp.meta_line - icon) * 0.5,
                icon,
                meta,
            );
            let got: Vec<_> = hidden
                .shapes
                .iter()
                .filter(|sh| sh.bbox[1] >= row - 2.0)
                .cloned()
                .collect();
            assert_eq!(got, want, "{scale}×: the bottom row's shapes");
            // The branch's floor is the padding box's left: its budget is every whole
            // column of the row less its icon lead, not what a status would leave.
            let room = (inp.rect[2] - 2.0 * (1.0 + 16.0) * s) - lead;
            assert_eq!(
                cols(&branch.text),
                (room / inp.meta_col).floor() as usize,
                "{scale}×: the branch fills the row from the left"
            );

            // The control: a drawable status with the toggle on does draw, so this test
            // cannot pass on a `build_card` that ignores the status altogether.
            let mut working = input_at(scale);
            working.show_status = true;
            working.status = CardStatus::Working;
            let working = build_card(&working);
            assert_ne!(working, hidden, "{scale}×");
            assert!(run(&working, "Working").is_some(), "{scale}×");
        }
    }

    // ---- D14: budgets -------------------------------------------------------------------

    #[test]
    fn the_title_stops_short_of_the_icon_or_the_button() {
        // R13.1.
        let mut inp = input(AgentSkin::Claude, CardState::Default);
        inp.title = "a very long task title that would run across the whole card";
        let d = build_card(&inp);
        let crab_left = cells(&d).iter().map(|s| s.bbox[0]).fold(f32::MAX, f32::min);
        let t = &d.runs[0];
        assert_eq!(t.face, CardFace::Title);
        assert!(t.text.ends_with('…'));
        assert!(run_right(t, inp.title_col) <= crab_left - 20.0);
        inp.show_menu = true;
        let d = build_card(&inp);
        let plate = backplate(&d).expect("plate").bbox[0];
        assert!(run_right(&d.runs[0], inp.title_col) <= plate - 20.0);
        assert_eq!(d.title_zone[0] + d.title_zone[2], plate - 20.0);
    }

    #[test]
    fn the_folder_text_starts_after_its_icon() {
        // R13.2 — at inner-left + 38 (13 + 6 logical at 2×), within the title budget
        // less that.
        let mut inp = input(AgentSkin::Claude, CardState::Default);
        inp.folder = "~/a/very/deep/directory/structure/that/goes/on/and/on/terminal";
        let d = build_card(&inp);
        let f = d
            .runs
            .iter()
            .find(|r| r.text.ends_with("terminal"))
            .expect("folder");
        assert_eq!(f.x, LEFT + 38.0);
        assert!(f.text.starts_with('…'), "cut from the left");
        let title_right = d.title_zone[0] + d.title_zone[2];
        assert!(run_right(f, inp.meta_col) <= title_right);
        assert_eq!(
            f.top,
            TOP + line_px(34.0) + 6.0,
            "under the title, 3 logical apart"
        );
    }

    fn narrow(skin: AgentSkin, state: CardState) -> CardInput<'static> {
        // A 260-logical sidebar at 2×: card w = 520 − 56.
        let mut inp = input(skin, state);
        inp.rect = [28.0, 100.0, 464.0, 208.0];
        inp
    }

    #[test]
    fn the_status_is_whole_and_the_branch_yields() {
        // R13.3 — at the minimum width `Waiting for input` is drawn in full and the
        // branch shortens with an ellipsis.
        let mut inp = narrow(AgentSkin::Claude, CardState::Default);
        inp.status = CardStatus::WaitingForInput;
        inp.branch = Some("claude/eloquent-knuth");
        let d = build_card(&inp);
        let status = run(&d, "Waiting for input").expect("status in full");
        let branch = d
            .runs
            .iter()
            .find(|r| r.text.starts_with("cl"))
            .expect("branch");
        assert!(branch.text.ends_with('…'), "{:?}", branch.text);
        assert!(
            branch.x >= run_right(status, inp.meta_col) + 20.0,
            "no overlap"
        );
        assert_eq!(
            run_right(branch, inp.meta_col),
            28.0 + 464.0 - 34.0,
            "right-aligned"
        );
    }

    #[test]
    fn a_branch_with_no_room_is_dropped() {
        // …and when not even its icon fits, the branch is omitted entirely.
        let mut inp = narrow(AgentSkin::Claude, CardState::Default);
        inp.status = CardStatus::Exited(2147483647);
        inp.rect = [28.0, 100.0, 360.0, 208.0];
        let d = build_card(&inp);
        assert!(run(&d, "Exited 2147483647").is_some());
        // Nothing else on the bottom row — no branch text, not even a clipped one …
        let row = BOTTOM - line_px(25.0);
        assert!(d
            .runs
            .iter()
            .all(|r| r.top != row || r.text == "Exited 2147483647"));
        // … and no branch icon right of the status.
        let meta = linear(SidebarTheme::DEFAULT.card_meta_fg, CARD_META_ALPHA);
        assert!(!d
            .shapes
            .iter()
            .any(|s| s.fill0 == meta && s.bbox[1] >= row - 2.0));
    }

    #[test]
    fn a_branch_with_room_for_only_its_icon_keeps_it_right_aligned() {
        let mut inp = input(AgentSkin::Claude, CardState::Default);
        inp.status = CardStatus::WaitingForInput;
        // Leave 26 physical after the status: the 24px icon fits, no column does.
        let status_end = LEFT + 24.0 + 12.0 + 17.0 * inp.meta_col;
        let right = status_end + 20.0 + 26.0;
        inp.rect[2] = right + 34.0 - 28.0;
        let d = build_card(&inp);
        assert!(run(&d, "feat/cards").is_none());
        let meta = linear(SidebarTheme::DEFAULT.card_meta_fg, CARD_META_ALPHA);
        let branch_icon_right = d
            .shapes
            .iter()
            .filter(|s| s.fill0 == meta && s.bbox[1] > 200.0)
            .map(|s| s.bbox[0] + s.bbox[2])
            .fold(f32::MIN, f32::max);
        assert!(
            (branch_icon_right - right).abs() < 3.0,
            "{branch_icon_right} vs {right}"
        );
    }

    #[test]
    fn the_narrowest_sidebar_keeps_every_zone_legible() {
        // R13.4 — a hovered Claude card at 260 logical: title ≥ 111 logical, folder
        // ≥ 92 logical (198 content − 42.5 slot, the crab as drawn at 2× (D11) − 8 − 26
        // − 10, then − 19).
        let mut inp = narrow(AgentSkin::Claude, CardState::Hover);
        inp.show_menu = true;
        inp.folder = "~/a/b/c";
        let d = build_card(&inp);
        let title_w = d.title_zone[2];
        assert!(title_w >= 111.0 * 2.0, "title budget {title_w}");
        assert!(
            title_w - 38.0 >= 92.0 * 2.0,
            "folder budget {}",
            title_w - 38.0
        );
        assert!(run(&d, "~/a/b/c").is_some(), "a short path is untouched");
    }

    // ---- D15 / D16 --------------------------------------------------------------------

    #[test]
    fn editing_leaves_the_title_to_the_field() {
        // D15 — no title run; the zone is still reported, and the folder stays.
        let mut inp = input(AgentSkin::Claude, CardState::Selected);
        inp.editing = true;
        inp.show_menu = true;
        let d = build_card(&inp);
        assert!(!d.runs.iter().any(|r| r.face == CardFace::Title));
        assert_eq!(d.title_zone[0], LEFT);
        // The narrower budget beside the `≡` cuts the path from the left; it is there.
        assert!(d.runs.iter().any(|r| r.text.ends_with("terminal")));
    }

    #[test]
    fn build_card_is_pure() {
        // R15.1 — no device, no queue, and the same answer twice.
        for skin in [AgentSkin::Claude, AgentSkin::Generic] {
            for state in [CardState::Default, CardState::Hover, CardState::Selected] {
                let inp = input(skin, state);
                assert_eq!(build_card(&inp), build_card(&inp));
            }
        }
    }
}

//! The ai-monitor panel's semantic model — plain data, no pixels, no formatting (D11).
//!
//! `app` fills this from a `TelemetrySnapshot`; [`crate::monitor_layout`] turns it into
//! shapes and text runs. The split exists because only the app knows what a tool call
//! *is* and only the renderer knows how wide the panel is — `dashboard.rs`'s 104-column
//! string assembly existed solely to paper over that gap, and a docked panel with a known
//! width removes the reason for it.
//!
//! Nothing here measures, truncates or lays out. A field is either a `String` the app has
//! already decided on or a [`ColorRole`] — **never a raw hex**, so the design's palette has
//! exactly one home and the app cannot invent a sixteenth green.
//!
//! `PartialEq` is load-bearing (R7.3, not decoration): the layer compares the incoming
//! model against the one it shaped last frame and skips re-shaping when they match. A
//! panel drag calls `redraw()` on every `CursorMoved`, so without that comparison a
//! 50-move drag re-lays-out all ten regions 50 times — the cost D2/G1 forbid.

/// The design's palette, as roles rather than colours (D11).
///
/// The app picks a *meaning* (`Magenta` = a write, `Rose` = a failure) and this enum owns
/// the hex, so re-tuning the design touches one file and cannot leave the app disagreeing
/// with the renderer about what "failure red" is. Values transcribed from the design
/// source recorded in the spec's Research section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorRole {
    /// Accent green `#7cb87c` — the header rule, an ok outcome.
    AccentGreen,
    /// Brighter accent green `#8fcb8f` — an emphasised accent value.
    AccentGreenBright,
    /// Live lime `#a9c94a` — the CONNECTED signal. Liveness is carried by colour, not
    /// motion (D2: the panel never animates).
    LiveLime,
    /// Teal `#6fb3b8` — READ, cache, usage bars.
    Teal,
    /// Amber `#d8a35c` — an active agent, a slow call, a stale session.
    Amber,
    /// Magenta `#c98bb0` — WRITE.
    Magenta,
    /// Rose `#d9647a` — a failure.
    Rose,
    /// Primary text `#e7e2d8`.
    #[default]
    TextPrimary,
    /// Secondary text `#d3cec4`.
    TextSecondary,
    /// Muted text `#a8a297`.
    TextMuted,
    /// Faint text `#6d6a62` — labels, gutters, the `+N more` marker.
    TextFaint,
    /// Card surface `#282824`.
    CardBg,
    /// Hairline rule `#34332e` — section-heading fills, card borders, gauge tracks.
    Hairline,
    /// Panel background `#232320`.
    PanelBg,
    /// Header band background `#1d1d1b`.
    HeaderBg,
    /// The panel's 1px left border `#3a3934` — the only thing separating two dark
    /// surfaces from each other, so it is a token in its own right rather than the
    /// hairline reused.
    PanelBorder,
}

impl ColorRole {
    /// The role's colour as packed `0xRRGGBB`, the form
    /// [`crate::sidebar_theme::linear`] converts for the GPU.
    pub fn packed(self) -> u32 {
        match self {
            ColorRole::AccentGreen => 0x007C_B87C,
            ColorRole::AccentGreenBright => 0x008F_CB8F,
            ColorRole::LiveLime => 0x00A9_C94A,
            ColorRole::Teal => 0x006F_B3B8,
            ColorRole::Amber => 0x00D8_A35C,
            ColorRole::Magenta => 0x00C9_8BB0,
            ColorRole::Rose => 0x00D9_647A,
            ColorRole::TextPrimary => 0x00E7_E2D8,
            ColorRole::TextSecondary => 0x00D3_CEC4,
            ColorRole::TextMuted => 0x00A8_A297,
            ColorRole::TextFaint => 0x006D_6A62,
            ColorRole::CardBg => 0x0028_2824,
            ColorRole::Hairline => 0x0034_332E,
            ColorRole::PanelBg => 0x0023_2320,
            ColorRole::HeaderBg => 0x001D_1D1B,
            ColorRole::PanelBorder => 0x003A_3934,
        }
    }
}

/// The status pill's three states (D5).
///
/// The design carries one CONNECTED pill; the dashboard distinguishes four detached cases
/// in prose. The pill compresses them to an at-a-glance badge and
/// [`SessionRow::note`] keeps the diagnostic wording — losing that wording would leave
/// "you have no Claude here" and "your Claude has not logged yet" looking identical, and
/// they are diagnosed completely differently.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PanelStatus {
    /// A live session writing to its log.
    Connected,
    /// A session that exists but has gone quiet.
    Idle,
    /// No Claude session is attached to this tab.
    #[default]
    NoSession,
}

impl PanelStatus {
    /// The pill's wording (R5.2). Kept here rather than in the layout so the three
    /// states and their three labels cannot drift apart.
    pub fn label(self) -> &'static str {
        match self {
            PanelStatus::Connected => "CONNECTED",
            PanelStatus::Idle => "IDLE",
            PanelStatus::NoSession => "NO SESSION",
        }
    }

    /// The colour that carries liveness (D2 — colour, never motion).
    pub fn role(self) -> ColorRole {
        match self {
            PanelStatus::Connected => ColorRole::LiveLime,
            PanelStatus::Idle => ColorRole::Amber,
            PanelStatus::NoSession => ColorRole::TextFaint,
        }
    }
}

/// The session identity line under the header: `ai-monitor · 8-char id · [Claude] · model`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionRow {
    pub name: String,
    /// The short (8-char) session id. Empty when nothing is attached.
    pub short_id: String,
    /// The provider tag, e.g. `[Claude]`.
    pub provider: String,
    pub model: String,
    /// The detached wording, verbatim (D5/R5.3) — stale-since, undated, "this tab's
    /// session has not written a log yet", or "no Claude session in this tab". `None`
    /// when attached. This is the panel's answer to its most common confusing state
    /// ("why is this empty"), which is why it survives the pill's compression.
    pub note: Option<String>,
}

/// One tile of the 3x2 stat grid (UPTIME / IDLE / ERRORS / IN·OUT / CACHE / CONTEXT).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatTile {
    pub label: String,
    pub value: String,
    /// The value's colour role. The label is always faint; only the number is coloured.
    pub color: ColorRole,
    /// Filled fraction of the tile's gauge track, `0.0..=1.0`. Only CONTEXT carries one.
    ///
    /// **`None` is not `Some(0.0)`** (R5.5). An unknown context window must omit the
    /// gauge entirely: an empty track is a claim ("you have used none of your window")
    /// that a model matching no `MODEL_WINDOWS` prefix does not entitle the panel to
    /// make. The layout emits no track at all for `None` and an empty track for
    /// `Some(0.0)`.
    pub gauge: Option<f32>,
}

impl StatTile {
    /// A gaugeless tile.
    pub fn new(label: impl Into<String>, value: impl Into<String>, color: ColorRole) -> StatTile {
        StatTile {
            label: label.into(),
            value: value.into(),
            color,
            gauge: None,
        }
    }

    /// Attach a gauge, clamping into `0.0..=1.0` and **dropping a non-finite fraction to
    /// `None`** rather than to `0.0`: a NaN means the app could not work the fraction out,
    /// which is exactly the R5.5 case where no gauge may be drawn.
    pub fn with_gauge(mut self, fraction: f32) -> StatTile {
        self.gauge = clamp01(fraction);
        self
    }
}

/// A card row — TASKS and ACTIVE AGENTS.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    /// The 3px left accent bar's role; `None` draws no bar.
    pub accent: Option<ColorRole>,
    pub title: String,
    /// The trailing value (a duration, a status word).
    pub value: String,
    /// The second line. A card reserves its height whether or not this is set — see the
    /// note on [`crate::monitor_layout::SectionMetrics`].
    pub detail: Option<String>,
}

/// A fixed-width leading label with its own colour — the `READ` / `WRITE` verb column.
/// Rendered in a fixed gutter, not at its own width, so the verbs line up as a column.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowLabel {
    pub text: String,
    pub color: ColorRole,
}

/// A list row — SKILLS, FILE ACTIVITY, RECENT CALLS.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    /// The verb gutter (R5.6: `WRITE` magenta, `READ` teal).
    pub label: Option<RowLabel>,
    /// An outcome mark (`✓` / `✗`), drawn in [`Row::value_color`] — the mark and the
    /// value it qualifies always agree on colour.
    pub glyph: Option<char>,
    pub text: String,
    /// The trailing value — a duration, a count. Empty draws nothing.
    pub value: String,
    /// The trailing value's role (R5.7: 32.8s amber, 1.8s neutral, 139ms green).
    pub value_color: ColorRole,
}

/// A usage bar — TOOL USAGE.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bar {
    pub name: String,
    pub count: u64,
    /// Width of the filled part as a fraction of the track, normalised by the app to the
    /// **top named tool** (R5.8 — `other` is in neither the ranking nor the denominator).
    pub fraction: f32,
}

impl Bar {
    /// Build a bar, clamping the fraction into `0.0..=1.0`; a non-finite one becomes an
    /// empty bar (the count still shows, so the row is not silently lost).
    pub fn new(name: impl Into<String>, count: u64, fraction: f32) -> Bar {
        Bar {
            name: name.into(),
            count,
            fraction: clamp01(fraction).unwrap_or(0.0),
        }
    }
}

/// The four body shapes the design uses. A section is exactly one of them; the variant
/// decides both the row's anatomy and its height.
#[derive(Clone, Debug, PartialEq)]
pub enum SectionBody {
    /// The empty-state wording, drawn **on the heading** rather than as a row — the same
    /// placement rule (and reason) as `+N more`: an empty state must not consume the
    /// one-row grant D18 guarantees a populated neighbour.
    Empty(String),
    /// TASKS / ACTIVE AGENTS.
    Cards(Vec<Card>),
    /// SKILLS / FILE ACTIVITY / RECENT CALLS.
    Rows(Vec<Row>),
    /// TOOL USAGE.
    Bars(Vec<Bar>),
}

impl Default for SectionBody {
    fn default() -> SectionBody {
        SectionBody::Empty(String::new())
    }
}

impl SectionBody {
    /// How many entries the body actually holds — the cap the height allocator honours
    /// (R8.3). `Empty` holds none, which is why it can never be granted a row.
    pub fn len(&self) -> usize {
        match self {
            SectionBody::Empty(_) => 0,
            SectionBody::Cards(v) => v.len(),
            SectionBody::Rows(v) => v.len(),
            SectionBody::Bars(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One of the six content sections, in the design's order (TASKS, ACTIVE AGENTS, SKILLS,
/// FILE ACTIVITY, RECENT CALLS, TOOL USAGE). The order is the app's — the layout draws
/// [`MonitorModel::sections`] as given, and the allocator degrades from its bottom.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub label: String,
    /// The session total, which may exceed `body.len()` — the app caps what it sends, and
    /// the difference between this and the rows actually drawn is what `+N more` reports
    /// (R8.6).
    pub total: usize,
    pub body: SectionBody,
}

/// Everything the panel draws (D10's ten regions).
///
/// `title`, `status`, `session`, `tiles` and `footer` are the **pinned chrome**: the layout
/// places them before any section, so the sections divide only what is left over.
///
/// "Pinned" is not the same as "unconditional", and R5.1 records the difference. The header,
/// the session row and the footer are the **floor** — they are drawn at every height a
/// window can usefully produce. The **stat grid is the one region that yields**: on a panel
/// too short to draw a tile row clear of the footer it drops whole rows of `TILE_COLS`, from
/// the bottom of the design's order, so CONTEXT goes before UPTIME. See
/// `monitor_layout::tests::r5_1_the_floor_survives_every_height_and_the_stat_grid_yields_whole_rows`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MonitorModel {
    pub visible: bool,
    /// The header title.
    pub title: String,
    pub status: PanelStatus,
    pub session: SessionRow,
    /// The stat grid, laid out three per row in the order given.
    pub tiles: Vec<StatTile>,
    pub sections: Vec<Section>,
    /// The footer line. An **absolute** last-update time (D9/D20) — a relative age would
    /// freeze and start lying the moment nothing repaints, which under D2 is most of the
    /// time.
    pub footer: String,
}

/// `Some(v)` clamped into `0.0..=1.0`, `None` for a non-finite input.
fn clamp01(v: f32) -> Option<f32> {
    v.is_finite().then(|| v.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_gauge_is_distinct_from_an_empty_one() {
        // R5.5 — the whole point of the Option: "we don't know your context window" and
        // "you have used 0% of it" must not compare equal, or the layout cannot tell
        // which one to draw.
        let unknown = StatTile::new("CONTEXT", "—", ColorRole::TextMuted);
        let empty = StatTile::new("CONTEXT", "0%", ColorRole::TextMuted).with_gauge(0.0);
        assert_eq!(unknown.gauge, None);
        assert_eq!(empty.gauge, Some(0.0));
        assert_ne!(unknown, empty);
    }

    #[test]
    fn gauge_clamps_and_refuses_a_non_finite_fraction() {
        assert_eq!(
            StatTile::new("CONTEXT", "45%", ColorRole::Teal)
                .with_gauge(0.45)
                .gauge,
            Some(0.45)
        );
        assert_eq!(
            StatTile::new("C", "", ColorRole::Teal)
                .with_gauge(4.0)
                .gauge,
            Some(1.0)
        );
        assert_eq!(
            StatTile::new("C", "", ColorRole::Teal)
                .with_gauge(-1.0)
                .gauge,
            Some(0.0)
        );
        // A NaN means "could not be computed", which is the R5.5 case — it must fall to
        // None (draw nothing), never to Some(0.0) (draw an empty track).
        assert_eq!(
            StatTile::new("C", "", ColorRole::Teal)
                .with_gauge(f32::NAN)
                .gauge,
            None
        );
        assert_eq!(Bar::new("bash", 76, f32::INFINITY).fraction, 0.0);
        assert_eq!(Bar::new("bash", 76, 2.5).fraction, 1.0);
    }

    #[test]
    fn roles_carry_the_designs_hexes() {
        // Spot-check the ones requirements name by colour: R5.6's verbs and R5.7's
        // slow-call amber.
        assert_eq!(ColorRole::Magenta.packed(), 0x00C9_8BB0);
        assert_eq!(ColorRole::Teal.packed(), 0x006F_B3B8);
        assert_eq!(ColorRole::Amber.packed(), 0x00D8_A35C);
        assert_eq!(ColorRole::PanelBg.packed(), 0x0023_2320);
        // Packed form is 0x00RRGGBB — a stray alpha byte would silently darken every
        // surface once `linear` unpacks it.
        for role in [
            ColorRole::AccentGreen,
            ColorRole::AccentGreenBright,
            ColorRole::LiveLime,
            ColorRole::Rose,
            ColorRole::TextPrimary,
            ColorRole::TextSecondary,
            ColorRole::TextMuted,
            ColorRole::TextFaint,
            ColorRole::CardBg,
            ColorRole::Hairline,
            ColorRole::HeaderBg,
            ColorRole::PanelBorder,
        ] {
            assert_eq!(role.packed() >> 24, 0, "{role:?} has a non-zero high byte");
        }
    }

    #[test]
    fn body_length_is_the_allocators_entry_cap() {
        // R8.3 counts entries, and an empty section holds none — so it can never be
        // granted a row, only a heading.
        assert_eq!(SectionBody::Empty("no tasks".into()).len(), 0);
        assert!(SectionBody::Empty("no tasks".into()).is_empty());
        assert_eq!(SectionBody::Cards(vec![Card::default(); 3]).len(), 3);
        assert_eq!(SectionBody::Rows(vec![Row::default(); 2]).len(), 2);
        assert_eq!(SectionBody::Bars(vec![Bar::new("b", 1, 1.0)]).len(), 1);
    }

    #[test]
    fn default_model_is_invisible_and_says_no_session() {
        // A model nobody filled must not claim a connection it cannot substantiate.
        let m = MonitorModel::default();
        assert!(!m.visible);
        assert_eq!(m.status, PanelStatus::NoSession);
        assert_eq!(m.status.label(), "NO SESSION");
        assert!(m.sections.is_empty() && m.tiles.is_empty());
    }

    #[test]
    fn equality_tracks_every_field_the_layer_shapes() {
        // R7.3 — the shaping cache skips work when the model compares equal, so a change
        // the panel *draws* must be a change PartialEq *sees*.
        let base = MonitorModel {
            visible: true,
            title: "ai-monitor".into(),
            footer: "Last update 16:21:18".into(),
            ..MonitorModel::default()
        };
        let mut changed = base.clone();
        assert_eq!(base, changed);
        changed.footer = "Last update 16:21:19".into();
        assert_ne!(base, changed);
        let mut status = base.clone();
        status.status = PanelStatus::Connected;
        assert_ne!(base, status);
    }
}

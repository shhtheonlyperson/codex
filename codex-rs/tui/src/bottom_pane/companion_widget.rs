use std::convert::TryFrom;
use std::time::Duration;
use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;

use crate::bottom_pane::ChatComposer;
use crate::frames::FRAME_TICK_DEFAULT;
use crate::render::renderable::Renderable;

const COMPANION_PLUGIN: &str = "companion";
const DEFAULT_RESERVE_COLUMNS: u16 = 20;
const MIN_COMPOSER_WIDTH: u16 = 36;
const SIDE_GAP: u16 = 2;
const MIN_FLOAT_WIDTH: u16 = 28;
const MIN_BANNER_WIDTH: u16 = 48;
const WIDE_BANNER_WIDTH: u16 = 76;
const DEFAULT_IDLE_FRAME_MS: u64 = 500;
const DEFAULT_REACTION_FRAME_MS: u64 = 180;
const DEFAULT_PET_FRAME_MS: u64 = 120;

#[derive(Clone)]
struct CompanionPresence {
    visible: bool,
    muted: bool,
    label: String,
    subtitle: Option<String>,
    badge: Option<String>,
    face: String,
    color: Option<String>,
    species: Option<String>,
    reserved_columns: u16,
    animation: Option<codex_protocol::protocol::PluginUiAnimation>,
    updated_at: Instant,
}

#[derive(Clone)]
struct CompanionReaction {
    text: String,
    kind: Option<String>,
    burst: bool,
    headline: String,
    expires_at: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CompanionMotionKind {
    Reaction,
    Pet,
}

#[derive(Clone, Copy)]
struct CompanionMotion {
    kind: CompanionMotionKind,
    started_at: Instant,
    expires_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompanionReactionLayout {
    Compact,
    Banner(BannerSize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BannerSize {
    Medium,
    Wide,
}

pub(crate) struct CompanionWidget {
    animations_enabled: bool,
    presence: Option<CompanionPresence>,
    reaction: Option<CompanionReaction>,
    motion: Option<CompanionMotion>,
}

impl CompanionWidget {
    pub(crate) fn new(animations_enabled: bool) -> Self {
        Self {
            animations_enabled,
            presence: None,
            reaction: None,
            motion: None,
        }
    }

    pub(crate) fn apply_events(
        &mut self,
        events: &[codex_protocol::protocol::PluginUiEvent],
    ) -> bool {
        let now = Instant::now();
        let mut changed = false;
        for event in events {
            match event {
                codex_protocol::protocol::PluginUiEvent::Presence {
                    plugin,
                    visible,
                    muted,
                    label,
                    subtitle,
                    badge,
                    face,
                    color,
                    species,
                    reserved_columns,
                    animation,
                } if plugin == COMPANION_PLUGIN => {
                    let label = label.clone().unwrap_or_else(|| "Companion".to_string());
                    let face = face.clone().unwrap_or_else(|| "o_o".to_string());
                    self.presence = Some(CompanionPresence {
                        visible: *visible,
                        muted: *muted,
                        label,
                        subtitle: subtitle.clone(),
                        badge: badge.clone(),
                        face,
                        color: color.clone(),
                        species: species.clone(),
                        reserved_columns: reserved_columns.unwrap_or(DEFAULT_RESERVE_COLUMNS),
                        animation: animation.clone(),
                        updated_at: now,
                    });
                    if !visible || *muted {
                        self.reaction = None;
                        self.motion = None;
                    }
                    changed = true;
                }
                codex_protocol::protocol::PluginUiEvent::Reaction {
                    plugin,
                    text,
                    kind,
                    ttl_ms,
                } if plugin == COMPANION_PLUGIN => {
                    let ttl = Duration::from_millis(ttl_ms.unwrap_or(10_000));
                    let kind = kind.clone();
                    self.reaction = Some(CompanionReaction {
                        text: text.clone(),
                        kind: kind.clone(),
                        burst: is_burst_kind(kind.as_deref()),
                        headline: burst_headline(kind.as_deref(), text),
                        expires_at: now + ttl,
                    });
                    self.motion = Some(CompanionMotion {
                        kind: CompanionMotionKind::Reaction,
                        started_at: now,
                        expires_at: now + ttl,
                    });
                    changed = true;
                }
                codex_protocol::protocol::PluginUiEvent::Pet { plugin, ttl_ms }
                    if plugin == COMPANION_PLUGIN =>
                {
                    let ttl = Duration::from_millis(ttl_ms.unwrap_or(2_500));
                    self.motion = Some(CompanionMotion {
                        kind: CompanionMotionKind::Pet,
                        started_at: now,
                        expires_at: now + ttl,
                    });
                    changed = true;
                }
                _ => {}
            }
        }
        changed
    }

    pub(crate) fn prune_expired(&mut self) -> bool {
        let now = Instant::now();
        let mut changed = false;
        if self
            .reaction
            .as_ref()
            .is_some_and(|reaction| reaction.expires_at <= now)
        {
            self.reaction = None;
            changed = true;
        }
        if self
            .motion
            .as_ref()
            .is_some_and(|motion| motion.expires_at <= now)
        {
            self.motion = None;
            changed = true;
        }
        changed
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.presence
            .as_ref()
            .is_some_and(|presence| presence.visible && !presence.muted)
    }

    pub(crate) fn reserve_columns(&self) -> u16 {
        self.presence
            .as_ref()
            .map(|presence| presence.reserved_columns.max(14))
            .unwrap_or(DEFAULT_RESERVE_COLUMNS)
    }

    pub(crate) fn can_render_sidecar(&self, width: u16) -> bool {
        self.is_visible()
            && width
                >= MIN_COMPOSER_WIDTH
                    .saturating_add(SIDE_GAP)
                    .saturating_add(self.reserve_columns())
    }

    pub(crate) fn desired_height_for_width(&self, width: u16, sidecar: bool) -> u16 {
        if !self.is_visible() || width == 0 {
            return 0;
        }
        let mut lines = 2;
        if self.reaction.is_some() || !sidecar {
            lines += 1;
        }
        lines.min(width.max(1))
    }

    pub(crate) fn should_render_float(&self, width: u16) -> bool {
        self.is_visible() && self.reaction.is_some() && width >= MIN_FLOAT_WIDTH
    }

    pub(crate) fn desired_float_height(&self, width: u16) -> u16 {
        if !self.should_render_float(width) {
            return 0;
        }
        Paragraph::new(self.reaction_lines(width))
            .wrap(Wrap { trim: false })
            .line_count(width)
            .try_into()
            .unwrap_or(0)
    }

    pub(crate) fn schedule_next_frame_delay(&self) -> Option<Duration> {
        if !self.is_visible() {
            return None;
        }
        let now = Instant::now();
        let mut next_delay = None;
        if let Some(motion) = self.motion {
            next_delay = Some(duration_until(now, motion.expires_at));
        } else if let Some(reaction) = &self.reaction {
            next_delay = Some(duration_until(now, reaction.expires_at));
        }
        if self.animations_enabled {
            if let Some((started_at, frame_ms, frames)) = self.active_frame_timeline() {
                if frames.len() > 1 && frame_ms > 0 {
                    let frame = Duration::from_millis(frame_ms);
                    let elapsed_ms = now.saturating_duration_since(started_at).as_millis();
                    let rem_ms = elapsed_ms % u128::from(frame_ms);
                    let delay_ms = if rem_ms == 0 {
                        frame_ms
                    } else {
                        frame_ms.saturating_sub(rem_ms as u64)
                    };
                    let rem = if delay_ms == 0 {
                        frame
                    } else {
                        Duration::from_millis(delay_ms)
                    };
                    next_delay = Some(next_delay.map_or(rem, |current| current.min(rem)));
                }
            }
            if self
                .reaction
                .as_ref()
                .is_some_and(|reaction| reaction.burst)
            {
                let tick_ms = FRAME_TICK_DEFAULT.as_millis();
                if tick_ms > 0 {
                    let elapsed_ms = now
                        .saturating_duration_since(self.banner_started_at())
                        .as_millis();
                    let rem_ms = elapsed_ms % tick_ms;
                    let delay_ms = if rem_ms == 0 {
                        tick_ms
                    } else {
                        tick_ms - rem_ms
                    };
                    if let Ok(delay_ms_u64) = u64::try_from(delay_ms) {
                        let rem = Duration::from_millis(delay_ms_u64.max(1));
                        next_delay = Some(next_delay.map_or(rem, |current| current.min(rem)));
                    }
                }
            }
        }
        next_delay
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, sidecar: bool) {
        if !self.is_visible() || area.is_empty() {
            return;
        }
        let lines = self.render_lines(sidecar);
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    pub(crate) fn render_float(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() || !self.should_render_float(area.width) {
            return;
        }
        Paragraph::new(self.reaction_lines(area.width))
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }

    fn render_lines(&self, sidecar: bool) -> Vec<Line<'static>> {
        let Some(presence) = self.presence.as_ref() else {
            return Vec::new();
        };
        let accent = parse_color(presence.color.as_deref()).unwrap_or(Color::Cyan);
        let frame = self
            .current_frame()
            .unwrap_or_else(|| presence.face.clone());

        let mut header = vec![
            Span::styled(
                frame,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                presence.label.clone(),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
        ];
        if let Some(badge) = presence.badge.as_ref() {
            header.push(Span::raw(" "));
            header.push(Span::styled(
                format!("[{badge}]"),
                Style::default().fg(accent),
            ));
        }

        let mut lines = vec![Line::from(header)];
        if let Some(reaction) = self.reaction.as_ref() {
            lines.push(Line::from(vec![Span::styled(
                format!("> {}", reaction.text),
                Style::default().fg(accent),
            )]));
        }

        let detail = presence
            .subtitle
            .clone()
            .or_else(|| presence.species.clone())
            .unwrap_or_else(|| "Companion active".to_string());
        if !sidecar || self.reaction.is_none() {
            lines.push(Line::from(vec![Span::styled(
                detail,
                Style::default().fg(Color::DarkGray),
            )]));
        }

        lines
    }

    fn reaction_lines(&self, width: u16) -> Vec<Line<'static>> {
        match self.reaction_layout(width) {
            Some(CompanionReactionLayout::Banner(size)) => self.banner_lines(width, size),
            Some(CompanionReactionLayout::Compact) => self.compact_float_lines(),
            None => Vec::new(),
        }
    }

    fn reaction_layout(&self, width: u16) -> Option<CompanionReactionLayout> {
        let reaction = self.reaction.as_ref()?;
        if !self.is_visible() || width < MIN_FLOAT_WIDTH {
            return None;
        }
        if !reaction.burst || width < MIN_BANNER_WIDTH {
            return Some(CompanionReactionLayout::Compact);
        }
        if width >= WIDE_BANNER_WIDTH {
            Some(CompanionReactionLayout::Banner(BannerSize::Wide))
        } else {
            Some(CompanionReactionLayout::Banner(BannerSize::Medium))
        }
    }

    fn compact_float_lines(&self) -> Vec<Line<'static>> {
        let Some(presence) = self.presence.as_ref() else {
            return Vec::new();
        };
        let Some(reaction) = self.reaction.as_ref() else {
            return Vec::new();
        };
        let accent = parse_color(presence.color.as_deref()).unwrap_or(Color::Cyan);
        let face = self
            .current_frame()
            .unwrap_or_else(|| presence.face.clone());
        vec![Line::from(vec![
            Span::styled(
                face,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{} says:", presence.label),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!("\"{}\"", reaction.text),
                Style::default().fg(accent).add_modifier(Modifier::ITALIC),
            ),
        ])]
    }

    fn banner_lines(&self, width: u16, size: BannerSize) -> Vec<Line<'static>> {
        let Some(presence) = self.presence.as_ref() else {
            return Vec::new();
        };
        let Some(reaction) = self.reaction.as_ref() else {
            return Vec::new();
        };
        let accent = parse_color(presence.color.as_deref()).unwrap_or(Color::Cyan);
        let width = width as usize;
        let phase = self.banner_phase();
        let title_rows = match size {
            BannerSize::Medium => medium_title_rows(&reaction.headline),
            BannerSize::Wide => wide_title_rows(&reaction.headline),
        };
        let detail = fit_to_width(
            &format!(
                "{}  {}: {}",
                self.current_frame()
                    .unwrap_or_else(|| presence.face.clone()),
                presence.label,
                reaction.text
            ),
            width,
        );
        let mut lines = Vec::with_capacity(title_rows.len() + 3);
        lines.push(rainbow_line(
            pattern_line(
                &format!(" {} ", presence.label.to_ascii_uppercase()),
                width,
                phase,
            ),
            phase,
            true,
        ));
        for (idx, row) in title_rows.into_iter().enumerate() {
            let wobble = if self.animations_enabled && width > 56 && phase % 2 == 1 {
                1
            } else {
                0
            };
            lines.push(rainbow_line(
                center_text(&row, width, wobble),
                phase + idx + 1,
                true,
            ));
        }
        lines.push(Line::from(vec![Span::styled(
            center_text(&detail, width, 0),
            Style::default()
                .fg(accent)
                .add_modifier(Modifier::BOLD | Modifier::ITALIC),
        )]));
        let footer = if let Some(kind) = reaction.kind.as_deref() {
            pattern_line(
                &format!(" {} / WOW / ", kind.to_ascii_uppercase()),
                width,
                phase + 2,
            )
        } else {
            pattern_line(" BUDDY WOW ", width, phase + 2)
        };
        lines.push(rainbow_line(footer, phase + 2, false));
        lines
    }

    fn banner_phase(&self) -> usize {
        if !self.animations_enabled {
            return 0;
        }
        let tick_ms = FRAME_TICK_DEFAULT.as_millis();
        if tick_ms == 0 {
            return 0;
        }
        let elapsed_ms = Instant::now()
            .saturating_duration_since(self.banner_started_at())
            .as_millis();
        (elapsed_ms / tick_ms) as usize
    }

    fn banner_started_at(&self) -> Instant {
        self.motion
            .map(|motion| motion.started_at)
            .or_else(|| self.presence.as_ref().map(|presence| presence.updated_at))
            .unwrap_or_else(Instant::now)
    }

    fn current_frame(&self) -> Option<String> {
        let presence = self.presence.as_ref()?;
        if !self.animations_enabled {
            return Some(presence.face.clone());
        }
        let (started_at, frame_ms, frames) = self.active_frame_timeline()?;
        if frames.is_empty() || frame_ms == 0 {
            return Some(presence.face.clone());
        }
        let elapsed_ms = Instant::now()
            .saturating_duration_since(started_at)
            .as_millis();
        let idx = ((elapsed_ms / u128::from(frame_ms)) % frames.len() as u128) as usize;
        frames
            .get(idx)
            .cloned()
            .or_else(|| Some(presence.face.clone()))
    }

    fn active_frame_timeline(&self) -> Option<(Instant, u64, Vec<String>)> {
        let presence = self.presence.as_ref()?;
        let animation = presence.animation.as_ref()?;
        match self.motion {
            Some(CompanionMotion {
                kind: CompanionMotionKind::Reaction,
                started_at,
                ..
            }) if !animation.reaction_frames.is_empty() => Some((
                started_at,
                animation
                    .reaction_frame_ms
                    .unwrap_or(DEFAULT_REACTION_FRAME_MS),
                animation.reaction_frames.clone(),
            )),
            Some(CompanionMotion {
                kind: CompanionMotionKind::Pet,
                started_at,
                ..
            }) if !animation.pet_frames.is_empty() => Some((
                started_at,
                animation.pet_frame_ms.unwrap_or(DEFAULT_PET_FRAME_MS),
                animation.pet_frames.clone(),
            )),
            _ if !animation.idle_frames.is_empty() => Some((
                presence.updated_at,
                animation.idle_frame_ms.unwrap_or(DEFAULT_IDLE_FRAME_MS),
                animation.idle_frames.clone(),
            )),
            _ => None,
        }
    }
}

fn is_burst_kind(kind: Option<&str>) -> bool {
    matches!(
        kind.map(str::trim),
        Some("pet" | "setup" | "rename" | "set-type" | "hatch" | "burst" | "celebrate")
    )
}

fn burst_headline(kind: Option<&str>, text: &str) -> String {
    match kind.map(str::trim) {
        Some("pet") => "PET PARTY".to_string(),
        Some("setup") | Some("hatch") => "NEW PAL".to_string(),
        Some("rename") => "NAME DROP".to_string(),
        Some("set-type") => "VIBE SHIFT".to_string(),
        Some("burst") | Some("celebrate") => "BUDDY WOW".to_string(),
        _ => text
            .split_whitespace()
            .take(2)
            .map(|part| {
                part.chars()
                    .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
                    .collect::<String>()
                    .to_ascii_uppercase()
            })
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn wide_title_rows(title: &str) -> Vec<String> {
    let mut rows = vec![String::new(), String::new(), String::new()];
    for ch in title.chars().map(normalize_banner_char) {
        let glyph = if ch == ' ' {
            [
                "     ".to_string(),
                "     ".to_string(),
                "     ".to_string(),
            ]
        } else {
            let brush = ch.to_string().repeat(3);
            [
                format!(" {brush} "),
                format!(" {ch} {ch} "),
                format!(" {brush} "),
            ]
        };
        for (row, piece) in rows.iter_mut().zip(glyph) {
            row.push_str(&piece);
        }
    }
    rows
}

fn medium_title_rows(title: &str) -> Vec<String> {
    let mut rows = vec![String::new(), String::new()];
    for ch in title.chars().map(normalize_banner_char) {
        let glyph = if ch == ' ' {
            ["   ".to_string(), "   ".to_string()]
        } else {
            let brush = ch.to_string().repeat(2);
            [format!(" {brush} "), format!(" {brush} ")]
        };
        for (row, piece) in rows.iter_mut().zip(glyph) {
            row.push_str(&piece);
        }
    }
    rows
}

fn normalize_banner_char(ch: char) -> char {
    if ch.is_ascii_alphanumeric() {
        ch.to_ascii_uppercase()
    } else if ch == '-' {
        '-'
    } else {
        ' '
    }
}

fn center_text(text: &str, width: usize, wobble: usize) -> String {
    let text = fit_to_width(text, width.saturating_sub(wobble));
    let text_width = text.chars().count();
    let remaining = width.saturating_sub(text_width);
    let left = remaining / 2 + wobble.min(remaining);
    let right = width.saturating_sub(text_width + left);
    format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
}

fn fit_to_width(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    if width <= 3 {
        return text.chars().take(width).collect();
    }
    let mut out = text.chars().take(width - 3).collect::<String>();
    out.push_str("...");
    out
}

fn pattern_line(label: &str, width: usize, phase: usize) -> String {
    let label = fit_to_width(label, width);
    let label_chars = label.chars().collect::<Vec<_>>();
    let pattern = ['=', '~', '*', '-'];
    let mut chars = (0..width)
        .map(|idx| pattern[(idx + phase) % pattern.len()])
        .collect::<Vec<_>>();
    if label_chars.len() <= width {
        let start = (width - label_chars.len()) / 2;
        for (idx, ch) in label_chars.into_iter().enumerate() {
            chars[start + idx] = ch;
        }
    }
    chars.into_iter().collect()
}

fn rainbow_line(text: String, phase: usize, bold: bool) -> Line<'static> {
    let palette = [
        Color::Magenta,
        Color::Red,
        Color::Yellow,
        Color::Green,
        Color::Cyan,
        Color::Blue,
    ];
    let spans = text
        .chars()
        .enumerate()
        .map(|(idx, ch)| {
            if ch == ' ' {
                Span::raw(" ".to_string())
            } else {
                let mut style = Style::default().fg(palette[(phase + idx) % palette.len()]);
                if bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                Span::styled(ch.to_string(), style)
            }
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

pub(crate) struct CompanionComposerLayout<'a> {
    composer: &'a ChatComposer,
    companion: &'a CompanionWidget,
}

impl<'a> CompanionComposerLayout<'a> {
    pub(crate) fn new(composer: &'a ChatComposer, companion: &'a CompanionWidget) -> Self {
        Self {
            composer,
            companion,
        }
    }
}

impl Renderable for CompanionComposerLayout<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() || !self.companion.is_visible() {
            self.composer.render(area, buf);
            return;
        }

        if self.companion.can_render_sidecar(area.width) {
            let reserve = self
                .companion
                .reserve_columns()
                .min(area.width.saturating_sub(MIN_COMPOSER_WIDTH));
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(MIN_COMPOSER_WIDTH),
                    Constraint::Length(SIDE_GAP),
                    Constraint::Length(reserve),
                ])
                .split(area);
            self.composer.render(chunks[0], buf);
            self.companion.render(chunks[2], buf, /*sidecar*/ true);
            return;
        }

        let companion_height = self
            .companion
            .desired_height_for_width(area.width, /*sidecar*/ false);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(companion_height), Constraint::Min(1)])
            .split(area);
        self.companion.render(chunks[0], buf, /*sidecar*/ false);
        self.composer.render(chunks[1], buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        if !self.companion.is_visible() {
            return self.composer.desired_height(width);
        }
        if self.companion.can_render_sidecar(width) {
            let reserve = self
                .companion
                .reserve_columns()
                .min(width.saturating_sub(1));
            let composer_width = width.saturating_sub(reserve.saturating_add(SIDE_GAP));
            return self.composer.desired_height(composer_width).max(
                self.companion
                    .desired_height_for_width(reserve, /*sidecar*/ true),
            );
        }
        self.companion
            .desired_height_for_width(width, /*sidecar*/ false)
            .saturating_add(self.composer.desired_height(width))
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if !self.companion.is_visible() || !self.companion.can_render_sidecar(area.width) {
            return self.composer.cursor_pos(area);
        }
        let reserve = self
            .companion
            .reserve_columns()
            .min(area.width.saturating_sub(MIN_COMPOSER_WIDTH));
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(MIN_COMPOSER_WIDTH),
                Constraint::Length(SIDE_GAP),
                Constraint::Length(reserve),
            ])
            .split(area);
        self.composer.cursor_pos(chunks[0])
    }
}

pub(crate) struct CompanionFloat<'a> {
    companion: &'a CompanionWidget,
}

impl<'a> CompanionFloat<'a> {
    pub(crate) fn new(companion: &'a CompanionWidget) -> Self {
        Self { companion }
    }
}

impl Renderable for CompanionFloat<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.companion.render_float(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.companion.desired_float_height(width)
    }
}

fn duration_until(now: Instant, target: Instant) -> Duration {
    target.checked_duration_since(now).unwrap_or_default()
}

fn parse_color(value: Option<&str>) -> Option<Color> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
        return None;
    }
    match value.to_ascii_lowercase().as_str() {
        "cyan" => Some(Color::Cyan),
        "blue" => Some(Color::Blue),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "red" => Some(Color::Red),
        "magenta" => Some(Color::Magenta),
        "white" => Some(Color::White),
        "gray" | "grey" => Some(Color::Gray),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::layout::Rect;

    #[test]
    fn reaction_enables_float_when_visible() {
        let mut widget = sample_widget(/*animations_enabled*/ false);
        widget.apply_events(&[reaction_event("hello", "note", 10_000)]);

        assert!(widget.should_render_float(60));
        assert!(widget.desired_float_height(60) >= 1);
    }

    #[test]
    fn wide_banner_shows_multiline_burst() {
        let mut widget = sample_widget(false);
        widget.apply_events(&[reaction_event("Tail velocity critical", "pet", 10_000)]);

        assert_eq!(
            widget.reaction_layout(90),
            Some(CompanionReactionLayout::Banner(BannerSize::Wide))
        );
        assert!(widget.desired_float_height(90) >= 5);

        let lines = render_float_lines(&widget, 90, 8);
        assert!(lines.iter().any(|line| line.contains("PPP")));
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Tail velocity critical"))
        );
    }

    #[test]
    fn narrow_width_falls_back_to_compact_float() {
        let mut widget = sample_widget(false);
        widget.apply_events(&[reaction_event("Tail velocity critical", "pet", 10_000)]);

        assert_eq!(
            widget.reaction_layout(42),
            Some(CompanionReactionLayout::Compact)
        );

        let lines = render_float_lines(&widget, 42, 3);
        assert!(lines.iter().any(|line| line.contains("Orbit says:")));
        assert!(lines.iter().all(|line| !line.contains("PPP")));
    }

    #[test]
    fn passive_reactions_stay_compact() {
        let mut widget = sample_widget(false);
        widget.apply_events(&[reaction_event("Nice work, captain.", "success", 10_000)]);

        assert_eq!(
            widget.reaction_layout(90),
            Some(CompanionReactionLayout::Compact)
        );
    }

    #[test]
    fn reduced_motion_keeps_banner_static() {
        let mut widget = sample_widget(false);
        widget.apply_events(&[reaction_event("Tail velocity critical", "pet", 10_000)]);

        assert_eq!(widget.banner_phase(), 0);
        let first = render_float_lines(&widget, 90, 8);
        std::thread::sleep(Duration::from_millis(120));
        let second = render_float_lines(&widget, 90, 8);
        assert_eq!(first, second);
    }

    #[test]
    fn burst_reaction_expires_cleanly() {
        let mut widget = sample_widget(false);
        widget.apply_events(&[reaction_event("Boom", "pet", 0)]);

        assert!(widget.prune_expired());
        assert_eq!(widget.desired_float_height(90), 0);
    }

    #[test]
    fn hidden_presence_clears_active_banner() {
        let mut widget = sample_widget(false);
        widget.apply_events(&[reaction_event("Boom", "pet", 10_000)]);
        widget.apply_events(&[codex_protocol::protocol::PluginUiEvent::Presence {
            plugin: COMPANION_PLUGIN.to_string(),
            visible: false,
            muted: false,
            label: Some("Orbit".to_string()),
            subtitle: Some("Campaigner".to_string()),
            badge: Some("ENFP".to_string()),
            face: Some("o_o".to_string()),
            color: Some("cyan".to_string()),
            species: Some("fox".to_string()),
            reserved_columns: Some(20),
            animation: None,
        }]);

        assert_eq!(widget.reaction_layout(90), None);
        assert_eq!(widget.desired_float_height(90), 0);
    }

    #[test]
    fn later_burst_reactions_replace_earlier_banner_text() {
        let mut widget = sample_widget(false);
        widget.apply_events(&[reaction_event("First spark", "pet", 10_000)]);
        widget.apply_events(&[reaction_event("Fresh vibe unlocked", "set-type", 10_000)]);

        let lines = render_float_lines(&widget, 90, 8);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Fresh vibe unlocked"))
        );
        assert!(lines.iter().all(|line| !line.contains("First spark")));
    }

    fn sample_widget(animations_enabled: bool) -> CompanionWidget {
        let mut widget = CompanionWidget::new(animations_enabled);
        widget.apply_events(&[codex_protocol::protocol::PluginUiEvent::Presence {
            plugin: COMPANION_PLUGIN.to_string(),
            visible: true,
            muted: false,
            label: Some("Orbit".to_string()),
            subtitle: Some("Campaigner".to_string()),
            badge: Some("ENFP".to_string()),
            face: Some("o_o".to_string()),
            color: Some("cyan".to_string()),
            species: Some("fox".to_string()),
            reserved_columns: Some(20),
            animation: None,
        }]);
        widget
    }

    fn reaction_event(
        text: &str,
        kind: &str,
        ttl_ms: u64,
    ) -> codex_protocol::protocol::PluginUiEvent {
        codex_protocol::protocol::PluginUiEvent::Reaction {
            plugin: COMPANION_PLUGIN.to_string(),
            text: text.to_string(),
            kind: Some(kind.to_string()),
            ttl_ms: Some(ttl_ms),
        }
    }

    fn render_float_lines(widget: &CompanionWidget, width: u16, height: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        widget.render_float(area, &mut buf);
        (0..height)
            .map(|row| {
                let mut line = String::new();
                for col in 0..width {
                    let symbol = buf[(col, row)].symbol();
                    if symbol.is_empty() {
                        line.push(' ');
                    } else {
                        line.push_str(symbol);
                    }
                }
                line.trim_end().to_string()
            })
            .collect()
    }
}

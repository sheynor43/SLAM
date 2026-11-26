//! The settings panel's "Tablet" section: OpenTabletDriver's area, output mode and
//! filters, edited through its daemon (ADR-0166). The rows follow [`Row::ALL`] on the
//! screen; their indices start at [`FIRST`].
//!
//! The rows come in groups under note headers: the state and buttons, the tablet and its
//! output mode, the tablet area with its preview, the screen area, the relative mode, and
//! the installed filters, each with its settings below it while it is on.
//!
//! The `slam-otd` worker does all I/O. The update thread sends it commands and takes its
//! events without blocking. Taking a new state and editing the draft allocate (the draft
//! copy, the row names, the JSON tree): that happens only on the player's own actions in
//! the open panel, like a navigation.

use std::sync::Arc;

use slam_otd::model::{ABSOLUTE_MODE, LINUX_ARTIST_MODE, RELATIVE_MODE};
use slam_otd::{
    AbsoluteFlag, Area, AreaKind, Command, Event, OtdService, ProfileView, RelativeField,
    SettingValue, Settings, Snapshot, short_type_name,
};
use slam_ui::i18n::Localizer;
use slam_ui::settings_screen::{
    AreaPreview, PREVIEW_LINES, RowKind, RowLabels, RowSpec, RowValue, RowWidget, SettingsScreen,
};

use super::rows::Row;

/// The index of the section: before the feedback section, the last.
pub const SECTION: usize = 8;
/// The section's name key.
pub const SECTION_KEY: &str = "settings-section-tablet";
/// The first row's index on the screen.
pub const FIRST: usize = Row::ALL.len();
/// Filters of a profile shown; more are left to OTD's own UX.
pub const MAX_FILTERS: usize = 6;
/// Settings of a filter shown.
pub const MAX_PARAMS: usize = 12;
/// Largest display area side and centre offered, in pixels of OTD's virtual screen.
const DISPLAY_MAX: f64 = 16384.0;
/// How long a command may take before the worker is given up for hung (a Windows pipe
/// read has no timeout): its thread is left blocked and a new one serves the next ask.
const WORKER_TIMEOUT_NS: u64 = 10_000_000_000;
/// Construction step of the filter setting rows: their shown decimals (zeros trimmed).
const FINE_STEP: f64 = 0.001;
/// Put before a filter setting's name, to show it belongs to the filter above.
const SETTING_MARK: &str = "· ";

/// One row of the section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtdRow {
    NotRunning,
    Unsupported,
    /// Opens OTD's download page; shown while no daemon answers.
    Download,
    NoTablet,
    Failed,
    Refresh,
    Apply,
    Revert,
    /// The tablet whose profile is edited; its name replaces the label.
    Tablet,
    NextTablet,
    OutputMode,
    TabletAreaHeader,
    Preview,
    /// The empty lines the preview spans after its own.
    PreviewPad(u8),
    TabletWidth,
    TabletHeight,
    TabletX,
    TabletY,
    TabletRotation,
    LockAspect,
    DisplayAreaHeader,
    DisplayWidth,
    DisplayHeight,
    DisplayX,
    DisplayY,
    Clipping,
    AreaLimiting,
    RelativeHeader,
    RelativeX,
    RelativeY,
    RelativeRotation,
    FiltersHeader,
    NoFilters,
    /// An installed filter: on or off; its name replaces the label.
    Filter(u8),
    /// A number setting `.1` of filter `.0`.
    ParamNumber(u8, u8),
    /// A flag setting `.1` of filter `.0`.
    ParamFlag(u8, u8),
}

/// The rows before the filters, without the preview's pads.
const HEAD: [OtdRow; 32] = {
    use OtdRow::*;
    [
        NotRunning,
        Unsupported,
        Download,
        NoTablet,
        Failed,
        Refresh,
        Apply,
        Revert,
        Tablet,
        NextTablet,
        OutputMode,
        TabletAreaHeader,
        Preview,
        // The pads go here.
        TabletWidth,
        TabletHeight,
        TabletX,
        TabletY,
        TabletRotation,
        LockAspect,
        DisplayAreaHeader,
        DisplayWidth,
        DisplayHeight,
        DisplayX,
        DisplayY,
        Clipping,
        AreaLimiting,
        RelativeHeader,
        RelativeX,
        RelativeY,
        RelativeRotation,
        FiltersHeader,
        NoFilters,
    ]
};
const HEAD_LEN: usize = HEAD.len();
/// Where the preview's pads go in [`HEAD`].
const PADS_AT: usize = 13;

/// Rows: the head, the preview's pads, and per filter its toggle and a number and a flag
/// row per setting.
const COUNT: usize = HEAD_LEN + (PREVIEW_LINES - 1) + MAX_FILTERS * (1 + 2 * MAX_PARAMS);

const fn all_rows() -> [OtdRow; COUNT] {
    let mut rows = [OtdRow::NotRunning; COUNT];
    let mut n = 0;
    let mut i = 0;
    while i < HEAD_LEN {
        if i == PADS_AT {
            let mut p = 0;
            while p < PREVIEW_LINES - 1 {
                rows[n] = OtdRow::PreviewPad(p as u8);
                n += 1;
                p += 1;
            }
        }
        rows[n] = HEAD[i];
        n += 1;
        i += 1;
    }
    let mut f = 0;
    while f < MAX_FILTERS {
        rows[n] = OtdRow::Filter(f as u8);
        n += 1;
        let mut s = 0;
        while s < MAX_PARAMS {
            rows[n] = OtdRow::ParamNumber(f as u8, s as u8);
            rows[n + 1] = OtdRow::ParamFlag(f as u8, s as u8);
            n += 2;
            s += 1;
        }
        f += 1;
    }
    assert!(n == COUNT, "COUNT matches the rows listed");
    rows
}

impl OtdRow {
    /// Every row in screen order.
    pub const ALL: [OtdRow; COUNT] = all_rows();

    /// The row at screen index `index`, if it is one of this section's.
    pub fn at(index: usize) -> Option<OtdRow> {
        index
            .checked_sub(FIRST)
            .and_then(|i| Self::ALL.get(i))
            .copied()
    }

    /// This row's screen index.
    pub fn index(self) -> usize {
        FIRST
            + Self::ALL
                .iter()
                .position(|r| *r == self)
                .expect("OtdRow::ALL lists every row")
    }

    fn is_note(self) -> bool {
        matches!(
            self,
            Self::NotRunning
                | Self::Unsupported
                | Self::NoTablet
                | Self::Failed
                | Self::Tablet
                | Self::TabletAreaHeader
                | Self::DisplayAreaHeader
                | Self::RelativeHeader
                | Self::FiltersHeader
                | Self::NoFilters
                | Self::PreviewPad(_)
        )
    }

    fn kind(self) -> RowKind {
        let slider = |min: f64, max: f64, step: f64| RowKind::Slider {
            min,
            max,
            step,
            value: min,
        };
        match self {
            _ if self.is_note() => RowKind::Note,
            Self::Download | Self::NextTablet | Self::Refresh | Self::Apply | Self::Revert => {
                RowKind::Button
            }
            // The built-in modes and "other" (a plugin's).
            Self::OutputMode => RowKind::Dropdown {
                options: mode_options().len() + 1,
                selected: 0,
            },
            Self::Preview => RowKind::Preview,
            Self::TabletRotation | Self::RelativeRotation => slider(-180.0, 180.0, 1.0),
            Self::TabletWidth | Self::TabletHeight | Self::TabletX | Self::TabletY => {
                slider(0.0, 1.0, 0.01)
            }
            Self::DisplayWidth | Self::DisplayHeight | Self::DisplayX | Self::DisplayY => {
                slider(0.0, DISPLAY_MAX, 1.0)
            }
            Self::RelativeX | Self::RelativeY => slider(0.0, 100.0, 0.1),
            Self::ParamNumber(..) => slider(0.0, 1.0, FINE_STEP),
            _ => RowKind::Toggle { value: false },
        }
    }

    fn label_key(self) -> &'static str {
        match self {
            Self::NotRunning => "settings-otd-not-running",
            Self::Unsupported => "settings-otd-unsupported",
            Self::Download => "settings-otd-download",
            Self::NoTablet => "settings-otd-no-tablet",
            Self::Failed => "settings-otd-failed",
            Self::Tablet => "settings-otd-tablet",
            Self::NextTablet => "settings-otd-next-tablet",
            Self::Refresh => "settings-otd-refresh",
            Self::Apply => "settings-otd-apply",
            Self::Revert => "settings-otd-revert",
            Self::OutputMode => "settings-otd-output-mode",
            Self::TabletAreaHeader => "settings-otd-tablet-area",
            Self::Preview | Self::PreviewPad(_) => "settings-otd-preview",
            Self::TabletWidth => "settings-otd-width",
            Self::TabletHeight => "settings-otd-height",
            Self::TabletX => "settings-otd-x",
            Self::TabletY => "settings-otd-y",
            Self::TabletRotation => "settings-otd-rotation",
            Self::LockAspect => "settings-otd-lock-aspect",
            Self::DisplayAreaHeader => "settings-otd-display-area",
            Self::DisplayWidth => "settings-otd-width",
            Self::DisplayHeight => "settings-otd-height",
            Self::DisplayX => "settings-otd-x",
            Self::DisplayY => "settings-otd-y",
            Self::Clipping => "settings-otd-clipping",
            Self::AreaLimiting => "settings-otd-area-limiting",
            Self::RelativeHeader => "settings-otd-relative",
            Self::RelativeX => "settings-otd-relative-x",
            Self::RelativeY => "settings-otd-relative-y",
            Self::RelativeRotation => "settings-otd-rotation",
            Self::FiltersHeader => "settings-otd-filters",
            Self::NoFilters => "settings-otd-no-filters",
            Self::Filter(_) => "settings-otd-filter",
            Self::ParamNumber(..) | Self::ParamFlag(..) => "settings-otd-filter-setting",
        }
    }

    fn button_key(self) -> Option<&'static str> {
        Some(match self {
            Self::Download => "settings-otd-download-button",
            Self::NextTablet => "settings-otd-next-tablet-button",
            Self::Refresh => "settings-otd-refresh-button",
            Self::Apply => "settings-otd-apply-button",
            Self::Revert => "settings-otd-revert-button",
            _ => return None,
        })
    }

    fn unit_key(self) -> Option<&'static str> {
        Some(match self {
            Self::TabletWidth | Self::TabletHeight | Self::TabletX | Self::TabletY => {
                "settings-unit-mm"
            }
            Self::DisplayWidth | Self::DisplayHeight | Self::DisplayX | Self::DisplayY => {
                "settings-unit-px"
            }
            Self::TabletRotation | Self::RelativeRotation => "settings-unit-degrees",
            _ => return None,
        })
    }

    /// The row's name comes from the daemon's data.
    fn dynamic(self) -> bool {
        matches!(
            self,
            Self::Tablet | Self::Filter(_) | Self::ParamNumber(..) | Self::ParamFlag(..)
        )
    }

    /// Rows that edit the draft: disabled while a command is in flight.
    fn edits(self) -> bool {
        !self.is_note()
            && !matches!(
                self,
                Self::Download | Self::Refresh | Self::Apply | Self::Revert | Self::Preview
            )
    }
}

/// The output modes offered, in option order: the built-in ones of this platform.
fn mode_options() -> &'static [&'static str] {
    slam_otd::builtin_output_modes()
}

fn mode_key(path: &str) -> &'static str {
    match path {
        ABSOLUTE_MODE => "settings-otd-mode-absolute",
        RELATIVE_MODE => "settings-otd-mode-relative",
        LINUX_ARTIST_MODE => "settings-otd-mode-artist",
        _ => "settings-otd-mode-other",
    }
}

/// The section's row specs, in screen order.
pub fn specs() -> impl Iterator<Item = RowSpec> {
    OtdRow::ALL.iter().map(|row| RowSpec {
        section: SECTION,
        kind: row.kind(),
        restart_required: false,
    })
}

/// The section's row texts in the language of `localizer`.
pub fn labels(localizer: &Localizer) -> impl Iterator<Item = RowLabels> + '_ {
    let text = |key: &str| localizer.format(key, None).into_owned();
    OtdRow::ALL.iter().map(move |row| {
        let options = if *row == OtdRow::OutputMode {
            mode_options()
                .iter()
                .map(|m| text(mode_key(m)))
                .chain(std::iter::once(text("settings-otd-mode-other")))
                .collect()
        } else {
            Vec::new()
        };
        RowLabels {
            label: match row {
                OtdRow::Preview | OtdRow::PreviewPad(_) => String::new(),
                _ => text(row.label_key()),
            },
            unit: row.unit_key().map(text).unwrap_or_default(),
            options,
            button: row.button_key().map(text).unwrap_or_default(),
            shows_value: false,
            trim_zeros: matches!(
                row.kind(),
                RowKind::Slider { step, .. } if step < 1.0
            ),
            zero_text: String::new(),
            hint: String::new(),
            dynamic_label: row.dynamic(),
        }
    })
}

/// What the section searches in, per row: like the other rows'.
pub fn search_keys() -> impl Iterator<Item = &'static str> {
    OtdRow::ALL.iter().map(|row| row.label_key())
}

/// Where the daemon stands, as last heard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    /// Nothing heard yet.
    Unknown,
    NotRunning,
    Unsupported,
    /// The last command failed; a draft taken before stays editable.
    Failed,
    Ready,
}

/// What a button press asks of the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressed {
    Nothing,
    /// Open OTD's download page.
    Download,
}

/// The section's state on the update thread.
pub struct OtdSection {
    /// Whether the section talks to the daemon; off in unit tests, which must not reach a
    /// daemon running on the machine.
    enabled: bool,
    /// The daemon was asked since the panel opened.
    asked: bool,
    /// When the update thread first saw the worker busy with the commands in flight.
    busy_since: Option<u64>,
    service: Option<OtdService>,
    status: Status,
    snapshot: Option<Box<Snapshot>>,
    /// The settings being edited; `SetSettings` sends them on Apply.
    draft: Option<Settings>,
    dirty: bool,
    /// The connected tablet edited, an index into the snapshot's tablets.
    tablet: usize,
    /// Number settings' slider ranges per filter, fixed when the state arrives so dragging
    /// does not move them.
    ranges: [[(f64, f64, f64); MAX_PARAMS]; MAX_FILTERS],
    /// Per screen row: the daemon's names (empty: the static label).
    names: Arc<[Box<str>]>,
}

impl Default for OtdSection {
    fn default() -> Self {
        Self {
            enabled: !cfg!(test),
            asked: false,
            busy_since: None,
            service: None,
            status: Status::Unknown,
            snapshot: None,
            draft: None,
            dirty: false,
            tablet: 0,
            ranges: [[(0.0, 1.0, 0.01); MAX_PARAMS]; MAX_FILTERS],
            names: Arc::from([]),
        }
    }
}

impl OtdSection {
    /// The settings panel opened: the daemon is asked again once the section shows.
    pub fn panel_opened(&mut self) {
        self.asked = false;
    }

    /// The section shows: asks the daemon once per opening of the panel. Does not
    /// allocate after the first time (the worker thread).
    pub fn section_shown(&mut self) {
        if !self.asked {
            self.asked = true;
            // Unapplied edits stay: "Refresh" reads the daemon again on purpose.
            if !self.dirty {
                self.ask();
            }
        }
    }

    /// Starts the worker if needed and asks for the daemon's state. Allocates on the first
    /// call (the thread).
    fn ask(&mut self) {
        if !self.enabled {
            return;
        }
        if self.service.is_none() {
            match OtdService::spawn() {
                Ok(service) => self.service = Some(service),
                Err(e) => {
                    tracing::warn!("OpenTabletDriver worker: {e}");
                    self.status = Status::Failed;
                    return;
                }
            }
        }
        if !self.busy() {
            self.send(Command::Refresh);
        }
    }

    /// Sends `command`; a stopped worker is dropped, and the next ask starts another.
    fn send(&mut self, command: Command) {
        if let Some(service) = &mut self.service
            && !service.send(command)
        {
            tracing::warn!("OpenTabletDriver worker stopped");
            self.service = None;
            self.status = Status::Failed;
        }
    }

    fn busy(&self) -> bool {
        self.service.as_ref().is_some_and(OtdService::is_busy)
    }

    /// Takes the worker's events at `now_ns`; gives a worker busy for too long up. Returns
    /// whether the rows need a refresh. Does not allocate without news.
    pub fn poll(&mut self, now_ns: u64) -> bool {
        let mut changed = false;
        while let Some(event) = self.service.as_mut().and_then(OtdService::try_event) {
            changed = true;
            self.busy_since = None;
            self.take(event);
        }
        if self.busy() {
            let since = *self.busy_since.get_or_insert(now_ns);
            if now_ns.saturating_sub(since) > WORKER_TIMEOUT_NS {
                tracing::warn!("OpenTabletDriver did not answer; giving the worker up");
                // Dropping the handle detaches the blocked thread; it ends with the pipe.
                self.service = None;
                self.busy_since = None;
                self.status = Status::Failed;
                changed = true;
            }
        } else {
            self.busy_since = None;
        }
        changed
    }

    /// Takes one worker event.
    fn take(&mut self, event: Event) {
        match event {
            Event::NotRunning => self.status = Status::NotRunning,
            Event::Unsupported { version } => {
                tracing::info!("OpenTabletDriver {version} is not supported");
                self.status = Status::Unsupported;
            }
            Event::Failed(_) => self.status = Status::Failed,
            Event::Snapshot(snapshot) | Event::Applied(snapshot) => {
                self.status = Status::Ready;
                self.draft = Some(snapshot.settings.clone());
                self.dirty = false;
                self.tablet = self.tablet.min(snapshot.tablets.len().saturating_sub(1));
                self.snapshot = Some(snapshot);
                self.renew();
            }
        }
    }

    /// The edited profile's index in the draft and its view.
    fn profile(&self) -> Option<(usize, ProfileView)> {
        let tablet = self.snapshot.as_ref()?.tablets.get(self.tablet)?;
        let draft = self.draft.as_ref()?;
        let index = draft.profile_index(&tablet.name)?;
        Some((index, draft.profile(index)?))
    }

    fn surface(&self) -> [f32; 2] {
        self.snapshot
            .as_ref()
            .and_then(|s| s.tablets.get(self.tablet))
            .map_or([0.0, 0.0], |t| [t.width_mm, t.height_mm])
    }

    /// Fixes the filter settings' slider ranges and the row names for the profile shown.
    fn renew(&mut self) {
        let profile = self.profile().map(|(_, p)| p);
        if let Some(p) = &profile {
            for (f, filter) in p.filters.iter().take(MAX_FILTERS).enumerate() {
                for (s, setting) in filter.settings.iter().take(MAX_PARAMS).enumerate() {
                    self.ranges[f][s] = param_range(&setting.value);
                }
            }
        }
        self.names = self.row_names(profile.as_ref());
    }

    fn row_names(&self, profile: Option<&ProfileView>) -> Arc<[Box<str>]> {
        let mut names = vec![Box::<str>::from(""); FIRST + COUNT];
        let mut set = |row: OtdRow, text: &str| names[row.index()] = text.into();
        if let Some(t) = self
            .snapshot
            .as_ref()
            .and_then(|s| s.tablets.get(self.tablet))
        {
            set(OtdRow::Tablet, &t.name);
        }
        if let Some(p) = profile {
            for (f, filter) in p.filters.iter().take(MAX_FILTERS).enumerate() {
                set(OtdRow::Filter(f as u8), short_type_name(&filter.path));
                for (s, setting) in filter.settings.iter().take(MAX_PARAMS).enumerate() {
                    let name = format!("{SETTING_MARK}{}", setting.property);
                    set(OtdRow::ParamNumber(f as u8, s as u8), &name);
                    set(OtdRow::ParamFlag(f as u8, s as u8), &name);
                }
            }
        }
        names.into()
    }

    /// Shows the state on `screen`'s rows.
    pub fn refresh(&self, screen: &mut SettingsScreen) {
        // A new generation makes the draw thread lay the names out again: only for new ones.
        if !Arc::ptr_eq(screen.dynamic_labels(), &self.names) {
            screen.set_dynamic_labels(Arc::clone(&self.names));
        }
        let profile = self.profile();
        // A failed apply keeps the draft on screen, with the error note over it.
        let ready =
            self.snapshot.is_some() && matches!(self.status, Status::Ready | Status::Failed);
        let tablets = self.snapshot.as_ref().map_or(0, |s| s.tablets.len());
        let editing = ready && profile.is_some();
        let busy = self.busy();
        for row in OtdRow::ALL {
            let shown = match row {
                OtdRow::NotRunning => self.status == Status::NotRunning,
                OtdRow::Unsupported => self.status == Status::Unsupported,
                OtdRow::Download => {
                    matches!(self.status, Status::NotRunning | Status::Unsupported)
                }
                OtdRow::Failed => self.status == Status::Failed,
                OtdRow::NoTablet => ready && profile.is_none(),
                OtdRow::Refresh => true,
                OtdRow::NextTablet => editing && tablets > 1,
                _ => editing,
            };
            screen.set_hidden(row.index(), !shown);
            if row.edits() {
                // While a command is in flight the draft is not edited: an answer replaces it.
                screen.set_disabled(row.index(), busy);
            }
        }
        screen.set_disabled(OtdRow::Refresh.index(), busy);
        screen.set_disabled(OtdRow::Apply.index(), busy || !self.dirty);
        screen.set_disabled(OtdRow::Revert.index(), busy || !self.dirty);
        let Some((_, p)) = profile.filter(|_| editing) else {
            return;
        };

        // Output mode: the built-in options, then "other" for a plugin's mode.
        let modes = mode_options();
        let (options, selected) = match modes.iter().position(|m| *m == p.output_mode) {
            Some(at) => (modes.len(), at),
            None => (modes.len() + 1, modes.len()),
        };
        if let Some(RowWidget::Dropdown(d)) = screen.widget(OtdRow::OutputMode.index())
            && (d.count() != options || d.selected() != selected)
        {
            screen.set_dropdown(OtdRow::OutputMode.index(), options, selected);
        }

        self.refresh_areas(screen, &p);
        self.refresh_filters(screen, &p);
    }

    fn refresh_areas(&self, screen: &mut SettingsScreen, p: &ProfileView) {
        let absolute = p.absolute.as_ref().filter(|_| p.is_absolute());
        let relative = p.relative.filter(|_| !p.is_absolute());
        for row in OtdRow::ALL {
            let group = match row {
                OtdRow::TabletAreaHeader
                | OtdRow::Preview
                | OtdRow::PreviewPad(_)
                | OtdRow::TabletWidth
                | OtdRow::TabletHeight
                | OtdRow::TabletX
                | OtdRow::TabletY
                | OtdRow::TabletRotation
                | OtdRow::LockAspect
                | OtdRow::DisplayAreaHeader
                | OtdRow::DisplayWidth
                | OtdRow::DisplayHeight
                | OtdRow::DisplayX
                | OtdRow::DisplayY
                | OtdRow::Clipping
                | OtdRow::AreaLimiting => absolute.is_some(),
                OtdRow::RelativeHeader
                | OtdRow::RelativeX
                | OtdRow::RelativeY
                | OtdRow::RelativeRotation => relative.is_some(),
                _ => continue,
            };
            screen.set_hidden(row.index(), !group);
        }
        if let Some(a) = absolute {
            let surface = self.surface();
            let (w, h) = (f64::from(surface[0]), f64::from(surface[1]));
            let t = a.tablet;
            for (row, max, value) in [
                (OtdRow::TabletWidth, w, t.width),
                (OtdRow::TabletHeight, h, t.height),
                (OtdRow::TabletX, w, t.x),
                (OtdRow::TabletY, h, t.y),
            ] {
                fit(screen, row, (0.0, max, 0.01), value);
            }
            fit(
                screen,
                OtdRow::TabletRotation,
                (-180.0, 180.0, 1.0),
                t.rotation,
            );
            let d = a.display;
            for (row, value) in [
                (OtdRow::DisplayWidth, d.width),
                (OtdRow::DisplayHeight, d.height),
                (OtdRow::DisplayX, d.x),
                (OtdRow::DisplayY, d.y),
            ] {
                // Displays left of or above the main one have negative centres (Windows).
                fit(screen, row, (0.0, DISPLAY_MAX, 1.0), value);
            }
            screen.set_toggle(OtdRow::Clipping.index(), a.clipping);
            screen.set_toggle(OtdRow::AreaLimiting.index(), a.area_limiting);
            screen.set_toggle(OtdRow::LockAspect.index(), a.lock_aspect_ratio);
            screen.set_preview(AreaPreview {
                surface,
                centre: [t.x, t.y],
                size: [t.width, t.height],
                rotation_deg: t.rotation,
            });
        }
        if let Some(r) = relative {
            fit(
                screen,
                OtdRow::RelativeX,
                (0.0, 100.0, 0.1),
                r.x_sensitivity,
            );
            fit(
                screen,
                OtdRow::RelativeY,
                (0.0, 100.0, 0.1),
                r.y_sensitivity,
            );
            fit(
                screen,
                OtdRow::RelativeRotation,
                (-180.0, 180.0, 1.0),
                r.rotation,
            );
        }
    }

    /// The installed filters, each with its settings below it while it is on.
    fn refresh_filters(&self, screen: &mut SettingsScreen, p: &ProfileView) {
        screen.set_hidden(OtdRow::NoFilters.index(), !p.filters.is_empty());
        for f in 0..MAX_FILTERS {
            let filter = p.filters.get(f);
            let toggle = OtdRow::Filter(f as u8).index();
            screen.set_hidden(toggle, filter.is_none());
            if let Some(filter) = filter {
                screen.set_toggle(toggle, filter.enabled);
            }
            let open = filter.filter(|f| f.enabled);
            for s in 0..MAX_PARAMS {
                let number = OtdRow::ParamNumber(f as u8, s as u8).index();
                let flag = OtdRow::ParamFlag(f as u8, s as u8).index();
                let value = open.and_then(|f| f.settings.get(s)).map(|s| &s.value);
                screen.set_hidden(
                    number,
                    !matches!(value, Some(SettingValue::Float(_) | SettingValue::Int(_))),
                );
                screen.set_hidden(flag, !matches!(value, Some(SettingValue::Bool(_))));
                let range = self.ranges[f][s];
                match value {
                    Some(SettingValue::Float(v)) => fit64(screen, number, range, *v),
                    Some(SettingValue::Int(v)) => fit64(screen, number, range, *v as f64),
                    Some(SettingValue::Bool(b)) => screen.set_toggle(flag, *b),
                    _ => {}
                }
            }
        }
    }

    /// A row of the section changed to `value`: edits the draft.
    pub fn apply(&mut self, row: OtdRow, value: RowValue) {
        let Some((index, p)) = self.profile() else {
            return;
        };
        let Some(draft) = &mut self.draft else {
            return;
        };
        let number = match value {
            RowValue::Slider(v) => v as f32,
            _ => 0.0,
        };
        let flag = matches!(value, RowValue::Toggle(true));
        let result = match row {
            OtdRow::OutputMode => match value {
                RowValue::Choice(choice) => match mode_options().get(choice) {
                    Some(mode) => draft.set_output_mode(index, mode),
                    // "Other" keeps the plugin's mode.
                    None => Ok(()),
                },
                _ => Ok(()),
            },
            OtdRow::TabletWidth
            | OtdRow::TabletHeight
            | OtdRow::TabletX
            | OtdRow::TabletY
            | OtdRow::TabletRotation
            | OtdRow::DisplayWidth
            | OtdRow::DisplayHeight
            | OtdRow::DisplayX
            | OtdRow::DisplayY => {
                let Some(a) = &p.absolute else {
                    return;
                };
                let (kind, area) = edit_area(row, a.tablet, a.display, number, a.lock_aspect_ratio);
                draft.set_area(index, kind, area)
            }
            OtdRow::Clipping => draft.set_absolute_flag(index, AbsoluteFlag::Clipping, flag),
            OtdRow::AreaLimiting => {
                draft.set_absolute_flag(index, AbsoluteFlag::AreaLimiting, flag)
            }
            OtdRow::LockAspect => {
                draft.set_absolute_flag(index, AbsoluteFlag::LockAspectRatio, flag)
            }
            OtdRow::RelativeX => draft.set_relative(index, RelativeField::XSensitivity, number),
            OtdRow::RelativeY => draft.set_relative(index, RelativeField::YSensitivity, number),
            OtdRow::RelativeRotation => draft.set_relative(index, RelativeField::Rotation, number),
            OtdRow::Filter(f) => draft.set_filter_enabled(index, usize::from(f), flag),
            OtdRow::ParamNumber(f, s) => {
                let (f, s) = (usize::from(f), usize::from(s));
                let current = p
                    .filters
                    .get(f)
                    .and_then(|filter| filter.settings.get(s))
                    .map(|setting| &setting.value);
                let RowValue::Slider(v) = value else {
                    return;
                };
                let new = match current {
                    Some(SettingValue::Int(_)) => SettingValue::Int(v.round() as i64),
                    _ => SettingValue::Float(round_fine(v)),
                };
                draft.set_filter_value(index, f, s, new)
            }
            OtdRow::ParamFlag(f, s) => draft.set_filter_value(
                index,
                usize::from(f),
                usize::from(s),
                SettingValue::Bool(flag),
            ),
            _ => return,
        };
        match result {
            Ok(()) => self.dirty = true,
            Err(e) => tracing::warn!("OpenTabletDriver edit {row:?}: {e}"),
        }
    }

    /// A button row of the section was pressed.
    pub fn press(&mut self, row: OtdRow) -> Pressed {
        match row {
            OtdRow::Download => return Pressed::Download,
            OtdRow::Refresh => self.ask(),
            OtdRow::Apply => {
                if let Some(draft) = self.draft.clone().filter(|_| self.dirty) {
                    self.send(Command::Apply(Box::new(draft)));
                }
            }
            OtdRow::Revert => {
                self.draft = self.snapshot.as_ref().map(|s| s.settings.clone());
                self.dirty = false;
                self.renew();
            }
            OtdRow::NextTablet => {
                let count = self.snapshot.as_ref().map_or(0, |s| s.tablets.len());
                if count > 0 {
                    self.tablet = (self.tablet + 1) % count;
                }
                self.renew();
            }
            _ => {}
        }
        Pressed::Nothing
    }
}

/// Shows `value` on slider `row`, its range `(min, max, step)` widened to hold the value.
fn fit(screen: &mut SettingsScreen, row: OtdRow, range: (f64, f64, f64), value: f32) {
    fit64(screen, row.index(), range, f64::from(value));
}

fn fit64(screen: &mut SettingsScreen, row: usize, (min, max, step): (f64, f64, f64), value: f64) {
    screen.set_slider_range(row, min.min(value), max.max(value), step);
    screen.set_slider(row, value);
}

/// The area a size or centre row edits, with `value` set. With the aspect ratio locked,
/// a tablet side follows the other by the display's ratio, as OTD's UX does.
fn edit_area(row: OtdRow, tablet: Area, display: Area, value: f32, lock: bool) -> (AreaKind, Area) {
    let ratio = (display.width > 0.0 && display.height > 0.0)
        .then(|| display.height / display.width)
        .filter(|_| lock);
    let mut t = tablet;
    let mut d = display;
    match row {
        OtdRow::TabletWidth => {
            t.width = value;
            if let Some(r) = ratio {
                t.height = value * r;
            }
        }
        OtdRow::TabletHeight => {
            t.height = value;
            if let Some(r) = ratio {
                t.width = value / r;
            }
        }
        OtdRow::TabletX => t.x = value,
        OtdRow::TabletY => t.y = value,
        OtdRow::TabletRotation => t.rotation = value,
        OtdRow::DisplayWidth => d.width = value,
        OtdRow::DisplayHeight => d.height = value,
        OtdRow::DisplayX => d.x = value,
        OtdRow::DisplayY => d.y = value,
        _ => {}
    }
    match row {
        OtdRow::DisplayWidth | OtdRow::DisplayHeight | OtdRow::DisplayX | OtdRow::DisplayY => {
            (AreaKind::Display, d)
        }
        _ => (AreaKind::Tablet, t),
    }
}

/// A slider range for a filter setting the daemon gives no range for: from its value,
/// the next power of ten above twice its size.
fn param_range(value: &SettingValue) -> (f64, f64, f64) {
    match value {
        SettingValue::Int(v) => {
            let v = *v as f64;
            let top = decade(v.abs() * 2.0).max(10.0);
            (if v < 0.0 { -top } else { 0.0 }, top, 1.0)
        }
        SettingValue::Float(v) if (0.0..=1.0).contains(v) => (0.0, 1.0, 0.01),
        SettingValue::Float(v) => {
            let top = decade(v.abs() * 2.0);
            (if *v < 0.0 { -top } else { 0.0 }, top, top / 1000.0)
        }
        _ => (0.0, 1.0, 0.01),
    }
}

/// The smallest power of ten at or above `x` (1 for `x` ≤ 1).
fn decade(x: f64) -> f64 {
    if x <= 1.0 || !x.is_finite() {
        1.0
    } else {
        10f64.powf(x.log10().ceil())
    }
}

/// A slider's value without float noise: to the construction step.
fn round_fine(v: f64) -> f64 {
    (v / FINE_STEP).round() / (1.0 / FINE_STEP)
}

#[cfg(test)]
mod tests;

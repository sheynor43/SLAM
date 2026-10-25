//! The settings screen's display and resolution lists (ADR-0072). The window hands each
//! new display list to the `slam-displays` thread, which builds the lists for the update
//! thread and the dropdowns for the draw thread; both trade them in through rings and give
//! the old ones back to be freed there.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use slam_input::{Display, Displays};
use slam_ui::settings_screen::SettingsView;
use slam_ui::widgets::DropdownView;

/// The smallest size listed, as lazer.
const MIN_SIZE: [u16; 2] = [800, 600];

/// Lists and views in flight each way: display events come far apart.
const RING: usize = 4;

/// How often the thread frees what came back while no display event arrives.
const FREE_INTERVAL: Duration = Duration::from_millis(200);

/// One display's resolutions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SizeLists {
    /// The fullscreen options: `None` (the display's size) first, then the modes.
    pub fullscreen: Vec<Option<[u16; 2]>>,
    /// The windowed options.
    pub windowed: Vec<[u16; 2]>,
    /// Whether `windowed` has the window's size although no mode has it.
    pub custom_windowed: bool,
    /// The display's bounds and usable bounds (x, y, width, height in points), to fit a
    /// windowed size into (ADR-0073).
    pub bounds: [i32; 4],
    pub usable: [i32; 4],
}

impl SizeLists {
    /// The windowed options the modes give, as lazer counts them to show the row.
    pub fn windowed_modes(&self) -> usize {
        self.windowed.len() - usize::from(self.custom_windowed)
    }
}

/// What the update thread needs of the displays: one entry per display, in SDL's order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayLists {
    pub displays: Vec<SizeLists>,
    /// The index of the primary display.
    pub primary: usize,
    /// Counts the lists the thread built, from 1; the [`DisplayViews`] of the same lists
    /// carry the same number. 0 before the first.
    pub generation: u64,
}

impl DisplayLists {
    /// The display the settings screen shows for the `display` setting: the primary one
    /// when there is no such display, as the window opens there. `None` before the first
    /// list.
    pub fn shown(&self, display: u8) -> Option<usize> {
        let index = usize::from(display);
        match self.displays.len() {
            0 => None,
            len if index < len => Some(index),
            _ => Some(self.primary.min(self.displays.len() - 1)),
        }
    }
}

/// The draw thread's dropdowns for a [`DisplayLists`]: the display list, and each
/// display's two resolution lists. Generic for tests.
pub struct DisplayViews<V = DropdownView> {
    /// The [`DisplayLists::generation`] these show.
    pub generation: u64,
    pub display: V,
    pub fullscreen: Vec<V>,
    pub windowed: Vec<V>,
}

/// The sizes of `display`'s modes the settings screen lists, largest first.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Overlays/Settings/Sections/Graphics/LayoutSettings.cs
fn mode_sizes(display: &Display) -> Vec<[u16; 2]> {
    let mut sizes: Vec<[u16; 2]> = display
        .modes
        .iter()
        .copied()
        .filter(|[w, h]| *w >= MIN_SIZE[0] && *h >= MIN_SIZE[1])
        .collect();
    // Stable, as LINQ's OrderByDescending; Distinct then keeps the first of equal sizes.
    sizes.sort_by_key(|[w, h]| std::cmp::Reverse((*w).max(*h)));
    let mut distinct: Vec<[u16; 2]> = Vec::with_capacity(sizes.len());
    for size in sizes {
        if !distinct.contains(&size) {
            distinct.push(size);
        }
    }
    distinct
}

/// `display`'s fullscreen and windowed options. `windowed_size` joins the windowed ones
/// where it sorts when no mode has it, so the player's size stays shown, as lazer keeps it.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Overlays/Settings/Sections/Graphics/LayoutSettings.cs
pub fn size_lists(display: &Display, windowed_size: [u16; 2]) -> SizeLists {
    let sizes = mode_sizes(display);
    let usable = [display.usable[2], display.usable[3]];
    let mut windowed: Vec<[u16; 2]> = sizes
        .iter()
        .copied()
        .filter(|[w, h]| i32::from(*w) <= usable[0] && i32::from(*h) <= usable[1])
        .collect();
    let custom_windowed = !windowed.contains(&windowed_size);
    if custom_windowed {
        let key = windowed_size[0].max(windowed_size[1]);
        let at = windowed
            .iter()
            .position(|[w, h]| (*w).max(*h) < key)
            .unwrap_or(windowed.len());
        windowed.insert(at, windowed_size);
    }
    SizeLists {
        fullscreen: std::iter::once(None)
            .chain(sizes.into_iter().map(Some))
            .collect(),
        windowed,
        custom_windowed,
        bounds: display.bounds,
        usable: display.usable,
    }
}

/// A windowed `size` the player picked, fitted into the display's usable bounds below a
/// title bar `top` points high, and the windowed position that centres it there without
/// hiding the title bar (ADR-0073). Bottom, left and right borders are ignored, as lazer:
/// they are invisible padding.
// Ported from osu!lazer 2026.1005.0-lazer: osu.Game/Overlays/Settings/Sections/Graphics/LayoutSettings.cs (windowedResolution.BindValueChanged)
pub fn fit_windowed(
    size: [u16; 2],
    bounds: [i32; 4],
    usable: [i32; 4],
    top: f32,
) -> ([u16; 2], [f32; 2]) {
    let [bx, by, bw, bh] = bounds;
    let [ux, uy, uw, uh] = usable;
    let w = i32::from(size[0]).min(uw);
    let h = (f32::from(size[1]).min(uh as f32 - top)) as i32;
    let adjusted_y = (uy as f32 + (uh - h) as f32 / 2.0).max(uy as f32 + top);
    let y = if bh - h != 0 {
        (adjusted_y - by as f32) / (bh - h) as f32
    } else {
        0.0
    };
    let x = if bw - w != 0 {
        ((ux - bx) as f32 + (uw - w) as f32 / 2.0) / (bw - w) as f32
    } else {
        0.0
    };
    // The setting's own limits, as lazer's bindables clamp what is stored.
    let side = |v: i32, min: u16| v.clamp(i32::from(min), i32::from(slam_input::MAX_SIDE)) as u16;
    let [min_w, min_h] = slam_input::MIN_WINDOWED_SIZE;
    (
        [side(w, min_w), side(h, min_h)],
        slam_input::clamp_position([x, y]),
    )
}

/// The lists of every display in `displays`, numbered `generation`.
pub fn lists(displays: &Displays, generation: u64) -> DisplayLists {
    DisplayLists {
        displays: displays
            .list
            .iter()
            .map(|display| size_lists(display, displays.windowed_size))
            .collect(),
        primary: displays.primary,
        generation,
    }
}

/// Builds the draw thread's dropdowns; `None` in tests, without fonts.
pub type MakeViews = Box<dyn Fn(&Displays, &DisplayLists) -> Option<DisplayViews> + Send>;

/// The update thread's end.
pub struct ListsLink {
    lists: rtrb::Consumer<Box<DisplayLists>>,
    old: rtrb::Producer<Box<DisplayLists>>,
}

impl ListsLink {
    /// The newest lists, if any arrived; older ones go back. Does not allocate.
    pub fn take(&mut self) -> Option<Box<DisplayLists>> {
        let mut latest = None;
        while let Ok(lists) = self.lists.pop() {
            if let Some(older) = latest.replace(lists) {
                self.give_back(older);
            }
        }
        latest
    }

    /// Hands replaced lists to the `slam-displays` thread to free. With the ring full they
    /// are freed here: more display events than the ring holds within 200 ms.
    pub fn give_back(&mut self, lists: Box<DisplayLists>) {
        let _ = self.old.push(lists);
    }
}

/// The draw thread's end.
pub struct ViewsLink<V = DropdownView> {
    views: rtrb::Consumer<Box<DisplayViews<V>>>,
    old: rtrb::Producer<Box<DisplayViews<V>>>,
}

impl<V> ViewsLink<V> {
    /// The newest dropdowns, if any arrived; older ones go back. Does not allocate.
    pub fn take(&mut self) -> Option<Box<DisplayViews<V>>> {
        let mut latest = None;
        while let Ok(views) = self.views.pop() {
            if let Some(older) = latest.replace(views) {
                self.give_back(older);
            }
        }
        latest
    }

    /// Hands replaced dropdowns to the `slam-displays` thread to free, as
    /// [`ListsLink::give_back`].
    pub fn give_back(&mut self, views: Box<DisplayViews<V>>) {
        let _ = self.old.push(views);
    }
}

/// Where [`ShownViews`] puts dropdowns: the settings screen's rows.
pub trait DropdownRows<V> {
    /// Exchanges `view` with the dropdown of `row`; whether `row` is a dropdown.
    fn swap_dropdown(&mut self, row: usize, view: &mut V) -> bool;
}

impl DropdownRows<DropdownView> for SettingsView {
    fn swap_dropdown(&mut self, row: usize, view: &mut DropdownView) -> bool {
        SettingsView::swap_dropdown(self, row, view)
    }
}

/// The draw thread's display dropdowns and which of them the settings screen holds. The
/// screen holds the display list and the resolution lists of one display; their slots in
/// [`DisplayViews`] hold what the screen had before, so swapping again puts it back.
/// Dropdowns go on the screen only once the update thread shows the lists they were made
/// for, so texts and option counts match in every frame.
pub struct ShownViews<V = DropdownView> {
    link: ViewsLink<V>,
    /// On the screen, or about to be.
    views: Option<Box<DisplayViews<V>>>,
    /// Arrived, waiting for the update thread to show their lists.
    pending: Option<Box<DisplayViews<V>>>,
    /// The screen's rows: display, fullscreen size, windowed size.
    rows: [usize; 3],
    display_in: bool,
    /// The display whose resolution lists the screen holds.
    shown: Option<usize>,
}

impl<V> ShownViews<V> {
    pub fn new(link: ViewsLink<V>, rows: [usize; 3]) -> ShownViews<V> {
        ShownViews {
            link,
            views: None,
            pending: None,
            rows,
            display_in: false,
            shown: None,
        }
    }

    /// The display row's index on the screen.
    pub fn display_row(&self) -> usize {
        self.rows[0]
    }

    /// Takes the newest dropdowns, if any arrived, to wait for their lists. Call every
    /// frame, so that replaced ones go back soon. Does not allocate.
    pub fn receive(&mut self) {
        if let Some(new) = self.link.take()
            && let Some(older) = self.pending.replace(new)
        {
            self.link.give_back(older);
        }
    }

    /// Puts the dropdowns of the lists numbered `generation`, which the update thread
    /// shows, on `screen`, with the resolution lists of `display`, the one the screen's
    /// display row has chosen. Does not allocate.
    pub fn show(&mut self, screen: &mut impl DropdownRows<V>, generation: u64, display: usize) {
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.generation == generation)
        {
            self.take_back(screen);
            if let Some(old) = std::mem::replace(&mut self.views, self.pending.take()) {
                self.link.give_back(old);
            }
            if let Some(views) = &mut self.views {
                self.display_in = screen.swap_dropdown(self.rows[0], &mut views.display);
            }
        }
        if self
            .views
            .as_ref()
            .is_none_or(|v| v.generation != generation)
        {
            // The screen shows lists whose dropdowns have not arrived: keep its own.
            self.take_back(screen);
            return;
        }
        if self.shown == Some(display) {
            return;
        }
        self.swap_sizes(screen);
        self.shown = None;
        let Some(views) = &mut self.views else {
            return;
        };
        if let (Some(fullscreen), Some(windowed)) = (
            views.fullscreen.get_mut(display),
            views.windowed.get_mut(display),
        ) {
            screen.swap_dropdown(self.rows[1], fullscreen);
            screen.swap_dropdown(self.rows[2], windowed);
            self.shown = Some(display);
        }
    }

    /// Gives `screen` back the dropdowns it had before ours.
    fn take_back(&mut self, screen: &mut impl DropdownRows<V>) {
        self.swap_sizes(screen);
        self.shown = None;
        if self.display_in
            && let Some(views) = &mut self.views
        {
            screen.swap_dropdown(self.rows[0], &mut views.display);
        }
        self.display_in = false;
    }

    /// Swaps the resolution lists of the shown display with the screen's.
    fn swap_sizes(&mut self, screen: &mut impl DropdownRows<V>) {
        if let (Some(shown), Some(views)) = (self.shown, &mut self.views) {
            screen.swap_dropdown(self.rows[1], &mut views.fullscreen[shown]);
            screen.swap_dropdown(self.rows[2], &mut views.windowed[shown]);
        }
    }
}

/// The `slam-displays` thread; stops and joins on drop.
pub struct DisplaysThread {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for DisplaysThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The thread's ends of the rings.
struct Ends {
    lists: rtrb::Producer<Box<DisplayLists>>,
    old_lists: rtrb::Consumer<Box<DisplayLists>>,
    views: rtrb::Producer<Box<DisplayViews>>,
    old_views: rtrb::Consumer<Box<DisplayViews>>,
}

impl Ends {
    fn free_returned(&mut self) {
        while self.old_lists.pop().is_ok() {}
        while self.old_views.pop().is_ok() {}
    }
}

/// Starts the `slam-displays` thread. Display lists sent on the returned sender (from the
/// window's callback) come out as lists for the update thread and dropdowns for the draw
/// thread; a list equal to the previous one is skipped.
pub fn spawn(
    make_views: MakeViews,
) -> std::io::Result<(Sender<Displays>, ListsLink, ViewsLink, DisplaysThread)> {
    let (sender, receiver) = channel();
    let (lists, lists_rx) = rtrb::RingBuffer::new(RING);
    let (old_lists_tx, old_lists) = rtrb::RingBuffer::new(RING);
    let (views, views_rx) = rtrb::RingBuffer::new(RING);
    let (old_views_tx, old_views) = rtrb::RingBuffer::new(RING);
    let ends = Ends {
        lists,
        old_lists,
        views,
        old_views,
    };
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("slam-displays".to_owned())
        .spawn(move || run(&receiver, ends, &make_views, &flag))?;
    Ok((
        sender,
        ListsLink {
            lists: lists_rx,
            old: old_lists_tx,
        },
        ViewsLink {
            views: views_rx,
            old: old_views_tx,
        },
        DisplaysThread {
            stop,
            thread: Some(thread),
        },
    ))
}

fn run(receiver: &Receiver<Displays>, mut ends: Ends, make_views: &MakeViews, stop: &AtomicBool) {
    let mut previous: Option<Displays> = None;
    // A list not sent yet for full rings: sent once there is room, unless a newer comes.
    let mut waiting: Option<Displays> = None;
    let mut generation = 0;
    while !stop.load(Ordering::Relaxed) {
        ends.free_returned();
        match receiver.recv_timeout(FREE_INTERVAL) {
            Ok(displays) => waiting = Some(displays),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let Some(displays) = waiting.take() else {
            continue;
        };
        if previous.as_ref() == Some(&displays) {
            continue;
        }
        // Both or neither, so the texts drawn match the update thread's lists.
        if ends.lists.slots() == 0 || ends.views.slots() == 0 {
            waiting = Some(displays);
            continue;
        }
        generation += 1;
        let lists = lists(&displays, generation);
        if let Some(views) = make_views(&displays, &lists) {
            let _ = ends.views.push(Box::new(views));
        }
        let _ = ends.lists.push(Box::new(lists));
        previous = Some(displays);
    }
}

#[cfg(test)]
mod tests;

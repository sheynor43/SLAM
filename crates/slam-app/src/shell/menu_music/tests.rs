use super::*;

/// Records what the menu asks of it.
#[derive(Default)]
struct Fake {
    log: Vec<String>,
    generation: u32,
    ended: bool,
    /// The track's rate; `None` is 48 kHz.
    rate: Option<u32>,
}

impl Deck for Fake {
    fn load(&mut self, path: &Path) -> Option<u32> {
        self.generation += 1;
        self.ended = false;
        self.log.push(format!("load {}", path.display()));
        Some(self.generation)
    }
    fn seek(&mut self, frame: u64) {
        self.ended = false;
        self.log.push(format!("seek {frame}"));
    }
    fn play(&mut self) {
        self.log.push("play".into());
    }
    fn pause(&mut self) {
        self.log.push("pause".into());
    }
    fn fade(&mut self, volume: f32, delay_ms: f64, duration_ms: f64, easing: Easing) {
        self.log
            .push(format!("fade {volume} {delay_ms} {duration_ms} {easing:?}"));
    }
    fn ended(&self) -> bool {
        self.ended
    }
    fn sample_rate(&self) -> u32 {
        self.rate.unwrap_or(48_000)
    }
}

fn decks() -> [Fake; 2] {
    [Fake::default(), Fake::default()]
}

fn take(deck: &mut Fake) -> Vec<String> {
    std::mem::take(&mut deck.log)
}

const MS: u64 = 1_000_000;
/// 100 s at 48 kHz.
const LENGTH: u64 = 4_800_000;

fn preview(path: &str, preview_ms: i32) -> Want<'_> {
    Want::Preview {
        path: Path::new(path),
        preview_ms,
    }
}

fn none() -> Option<Picked> {
    None
}

#[test]
fn a_preview_from_silence_starts_before_its_point_and_rises() {
    let mut m = MenuMusic::default();
    let mut d = decks();
    m.tick(0, preview("a.mp3", 10_000), &mut d, none);
    assert_eq!(take(&mut d[1]), ["fade 0 0 0 None", "load a.mp3"]);
    m.loaded(1, 1, Some(LENGTH), &mut d);
    // 10 s minus 30 ms, at 48 kHz.
    assert_eq!(
        take(&mut d[1]),
        ["seek 478560", "play", "fade 1 0 800 OutQuint"]
    );
    // The same track wanted again changes nothing.
    m.tick(MS, preview("a.mp3", 10_000), &mut d, none);
    assert!(take(&mut d[1]).is_empty());
}

#[test]
fn the_preview_point_is_in_frames_of_the_decks_rate() {
    let mut m = MenuMusic::default();
    let mut d = decks();
    d[1].rate = Some(44_100);
    m.tick(0, preview("a.mp3", 10_000), &mut d, none);
    take(&mut d[1]);
    m.loaded(1, 1, Some(LENGTH), &mut d);
    // 10 s minus 30 ms, at 44.1 kHz.
    assert_eq!(take(&mut d[1])[0], "seek 439677");
}

#[test]
fn without_a_preview_time_the_point_is_at_two_fifths() {
    let mut m = MenuMusic::default();
    let mut d = decks();
    m.tick(0, preview("a.mp3", -1), &mut d, none);
    m.loaded(1, 1, Some(LENGTH), &mut d);
    assert_eq!(take(&mut d[1])[2], "seek 1918560");
    // Past the end counts as none.
    let mut m = MenuMusic::default();
    let mut d = decks();
    m.tick(0, preview("a.mp3", 200_000), &mut d, none);
    m.loaded(1, 1, Some(LENGTH), &mut d);
    assert_eq!(take(&mut d[1])[2], "seek 1918560");
}

#[test]
fn another_beatmap_crosses_over_from_the_old_track() {
    let mut m = MenuMusic::default();
    let mut d = decks();
    m.tick(0, preview("a.mp3", 0), &mut d, none);
    m.loaded(1, 1, Some(LENGTH), &mut d);
    take(&mut d[1]);
    m.tick(MS, preview("b.mp3", 0), &mut d, none);
    assert_eq!(take(&mut d[1]), ["fade 0 0 150 Out"]);
    assert_eq!(take(&mut d[0]), ["fade 0 0 0 None", "load b.mp3"]);
    m.loaded(0, 1, Some(LENGTH), &mut d);
    assert_eq!(take(&mut d[0]), ["seek 0", "play", "fade 1 30 220 None"]);
    // The old one pauses once faded.
    m.tick(100 * MS, preview("b.mp3", 0), &mut d, none);
    assert!(take(&mut d[1]).is_empty());
    m.tick(152 * MS, preview("b.mp3", 0), &mut d, none);
    assert_eq!(take(&mut d[1]), ["pause"]);
}

#[test]
fn silence_fades_out_and_the_preview_comes_back_where_it_stopped() {
    let mut m = MenuMusic::default();
    let mut d = decks();
    m.tick(0, preview("a.mp3", 0), &mut d, none);
    m.loaded(1, 1, Some(LENGTH), &mut d);
    take(&mut d[1]);
    m.tick(MS, Want::Silence, &mut d, none);
    m.tick(200 * MS, Want::Silence, &mut d, none);
    assert_eq!(take(&mut d[1]), ["fade 0 0 150 Out", "pause"]);
    m.tick(300 * MS, preview("a.mp3", 0), &mut d, none);
    assert_eq!(take(&mut d[1]), ["play", "fade 1 0 800 OutQuint"]);
}

#[test]
fn a_preview_loops_from_its_point() {
    let mut m = MenuMusic::default();
    let mut d = decks();
    m.tick(0, preview("a.mp3", 1_000), &mut d, none);
    m.loaded(1, 1, Some(LENGTH), &mut d);
    take(&mut d[1]);
    d[1].ended = true;
    m.tick(MS, preview("a.mp3", 1_000), &mut d, none);
    assert_eq!(take(&mut d[1]), ["seek 46560", "play"]);
}

#[test]
fn the_menu_picks_a_random_track_and_the_next_when_it_ends() {
    let mut m = MenuMusic::default();
    let mut d = decks();
    let picked = |name: &str| {
        let path = PathBuf::from(name);
        move || {
            Some(Picked {
                path,
                preview_ms: 10_000,
                at: (0, 0),
            })
        }
    };
    m.tick(0, Want::Menu, &mut d, picked("a.mp3"));
    assert_eq!(take(&mut d[1]), ["fade 0 0 0 None", "load a.mp3"]);
    m.loaded(1, 1, Some(LENGTH), &mut d);
    // 10 s minus 2.6 s, rising slowly; it does not loop.
    assert_eq!(
        take(&mut d[1]),
        ["seek 355200", "play", "fade 1 0 2600 InCubic"]
    );
    m.tick(MS, Want::Menu, &mut d, picked("unused.mp3"));
    assert!(take(&mut d[0]).is_empty());
    d[1].ended = true;
    m.tick(2 * MS, Want::Menu, &mut d, picked("b.mp3"));
    assert_eq!(take(&mut d[1]), ["pause"]);
    assert_eq!(take(&mut d[0]), ["fade 0 0 0 None", "load b.mp3"]);
    m.loaded(0, 1, Some(LENGTH), &mut d);
    // From its start, faded in.
    assert_eq!(take(&mut d[0]), ["play", "fade 1 30 220 None"]);
}

#[test]
fn the_menu_keeps_the_song_select_track() {
    let mut m = MenuMusic::default();
    let mut d = decks();
    m.tick(0, preview("a.mp3", 0), &mut d, none);
    m.loaded(1, 1, Some(LENGTH), &mut d);
    take(&mut d[1]);
    m.tick(MS, Want::Menu, &mut d, || panic!("nothing to pick"));
    assert!(take(&mut d[1]).is_empty());
}

#[test]
fn a_track_that_fails_is_not_tried_again_while_wanted() {
    let mut m = MenuMusic::default();
    let mut d = decks();
    m.tick(0, preview("bad.mp3", 0), &mut d, none);
    m.failed(MS, 1, 1, &mut d);
    take(&mut d[1]);
    m.tick(2 * MS, preview("bad.mp3", 0), &mut d, none);
    assert!(take(&mut d[0]).is_empty() && take(&mut d[1]).is_empty());
    // The menu waits before picking another.
    let mut picks = 0;
    m.tick(3 * MS, Want::Menu, &mut d, || {
        picks += 1;
        None
    });
    assert_eq!(picks, 0);
    m.tick(MS + RETRY_NS, Want::Menu, &mut d, || {
        picks += 1;
        None
    });
    assert_eq!(picks, 1);
}

#[test]
fn a_track_stopped_while_opening_waits_at_its_point_and_rises_later() {
    let mut m = MenuMusic::default();
    let mut d = decks();
    m.tick(0, preview("a.mp3", 1_000), &mut d, none);
    m.tick(MS, Want::Silence, &mut d, none);
    take(&mut d[1]);
    m.loaded(1, 1, Some(LENGTH), &mut d);
    assert_eq!(take(&mut d[1]), ["seek 46560"], "waits paused at its point");
    m.tick(2 * MS, preview("a.mp3", 1_000), &mut d, none);
    assert_eq!(take(&mut d[1]), ["play", "fade 1 0 800 OutQuint"]);
}

#[test]
fn a_track_wanted_again_before_it_opened_comes_in_as_planned() {
    let mut m = MenuMusic::default();
    let mut d = decks();
    m.tick(0, preview("a.mp3", 1_000), &mut d, none);
    m.tick(MS, Want::Silence, &mut d, none);
    m.tick(2 * MS, preview("a.mp3", 1_000), &mut d, none);
    take(&mut d[1]);
    m.loaded(1, 1, Some(LENGTH), &mut d);
    assert_eq!(
        take(&mut d[1]),
        ["seek 46560", "play", "fade 1 0 800 OutQuint"]
    );
}

#[test]
fn a_track_failing_while_it_plays_stops_and_is_not_looped() {
    let mut m = MenuMusic::default();
    let mut d = decks();
    m.tick(0, preview("a.mp3", 0), &mut d, none);
    m.loaded(1, 1, Some(LENGTH), &mut d);
    take(&mut d[1]);
    // A seek made generation 2; the failure is still this track's.
    m.failed(MS, 1, 2, &mut d);
    assert_eq!(take(&mut d[1]), ["pause"]);
    d[1].ended = true;
    m.tick(2 * MS, preview("a.mp3", 0), &mut d, none);
    assert!(take(&mut d[0]).is_empty() && take(&mut d[1]).is_empty());
}

#[test]
fn a_menu_track_the_song_select_selects_loops_and_back_in_the_menu_moves_on() {
    let mut m = MenuMusic::default();
    let mut d = decks();
    m.tick(0, Want::Menu, &mut d, || {
        Some(Picked {
            path: PathBuf::from("a.mp3"),
            preview_ms: 1_000,
            at: (0, 0),
        })
    });
    m.loaded(1, 1, Some(LENGTH), &mut d);
    take(&mut d[1]);
    m.tick(MS, preview("a.mp3", 1_000), &mut d, none);
    d[1].ended = true;
    m.tick(2 * MS, preview("a.mp3", 1_000), &mut d, none);
    assert_eq!(take(&mut d[1]), ["seek 46560", "play"]);
    // In the menu the preview no longer loops.
    m.tick(3 * MS, Want::Menu, &mut d, none);
    d[1].ended = true;
    m.tick(4 * MS, Want::Menu, &mut d, none);
    assert_eq!(take(&mut d[1]), ["pause"]);
}

/// A deck whose queue is full.
#[derive(Default)]
struct Full(Fake);

impl Deck for Full {
    fn load(&mut self, _: &Path) -> Option<u32> {
        self.0.log.push("load".into());
        None
    }
    fn seek(&mut self, frame: u64) {
        self.0.seek(frame);
    }
    fn play(&mut self) {
        self.0.play();
    }
    fn pause(&mut self) {
        self.0.pause();
    }
    fn fade(&mut self, volume: f32, delay_ms: f64, duration_ms: f64, easing: Easing) {
        self.0.fade(volume, delay_ms, duration_ms, easing);
    }
    fn ended(&self) -> bool {
        false
    }
    fn sample_rate(&self) -> u32 {
        self.0.sample_rate()
    }
}

#[test]
fn a_full_queue_is_not_tried_every_tick() {
    let mut m = MenuMusic::default();
    let mut d = [Full::default(), Full::default()];
    m.tick(0, preview("a.mp3", 0), &mut d, none);
    m.tick(MS, preview("a.mp3", 0), &mut d, none);
    let loads = |d: &[Full; 2]| d[1].0.log.iter().filter(|l| *l == "load").count();
    assert_eq!(loads(&d), 1);
    m.tick(RETRY_NS, preview("a.mp3", 0), &mut d, none);
    assert_eq!(loads(&d), 2);
}

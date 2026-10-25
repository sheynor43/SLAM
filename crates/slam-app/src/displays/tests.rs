use super::*;

fn display(modes: &[[u16; 2]], usable: [i32; 2]) -> Display {
    Display {
        name: "Test".to_owned(),
        bounds: [0, 0, 1920, 1080],
        usable: [0, 0, usable[0], usable[1]],
        modes: modes.to_vec(),
    }
}

#[test]
fn modes_are_filtered_sorted_by_the_longer_side_and_distinct() {
    let d = display(
        &[
            [1280, 1024],
            [1920, 1080],
            [640, 480],
            [800, 600],
            [1920, 1080],
            [1080, 1920],
            [1024, 768],
            [799, 600],
            [1920, 1200],
        ],
        [1920, 1080],
    );
    let lists = size_lists(&d, [1920, 1080]);
    assert_eq!(
        lists.fullscreen,
        vec![
            None,
            Some([1920, 1080]),
            Some([1080, 1920]),
            Some([1920, 1200]),
            Some([1280, 1024]),
            Some([1024, 768]),
            Some([800, 600]),
        ]
    );
}

#[test]
fn windowed_sizes_fit_the_usable_bounds() {
    let d = display(
        &[[2560, 1440], [1920, 1080], [1600, 900], [1280, 720]],
        [1920, 1050],
    );
    let lists = size_lists(&d, [1280, 720]);
    assert_eq!(lists.windowed, vec![[1600, 900], [1280, 720]]);
    assert!(!lists.custom_windowed);
    assert_eq!(lists.fullscreen.len(), 5);
}

#[test]
fn a_windowed_size_no_mode_has_joins_where_it_sorts() {
    let d = display(&[[1920, 1080], [1600, 900], [1280, 720]], [1920, 1080]);
    let lists = size_lists(&d, [1366, 768]);
    assert!(lists.custom_windowed);
    assert_eq!(lists.windowed_modes(), 3);
    assert_eq!(
        lists.windowed,
        vec![[1920, 1080], [1600, 900], [1366, 768], [1280, 720]]
    );
    // Smaller than every mode: last.
    let lists = size_lists(&d, [640, 480]);
    assert_eq!(lists.windowed.last(), Some(&[640, 480]));
    // Not a fullscreen option.
    assert!(!lists.fullscreen.contains(&Some([640, 480])));
}

#[test]
fn no_modes_leave_only_the_default_and_the_windowed_size() {
    let lists = size_lists(&display(&[], [1920, 1080]), [1366, 768]);
    assert_eq!(lists.fullscreen, vec![None]);
    assert_eq!(lists.windowed, vec![[1366, 768]]);
}

#[test]
fn a_missing_display_shows_as_the_primary_one() {
    let lists = DisplayLists {
        displays: vec![SizeLists::default(); 2],
        primary: 1,
        generation: 1,
    };
    assert_eq!(lists.shown(0), Some(0));
    assert_eq!(lists.shown(1), Some(1));
    assert_eq!(lists.shown(5), Some(1));
    assert_eq!(DisplayLists::default().shown(0), None);
}

#[test]
fn the_thread_skips_equal_lists_and_frees_returned_ones() {
    let (sender, mut lists_link, mut views_link, thread) =
        spawn(Box::new(|_, _| None)).expect("thread starts");
    let displays = Displays {
        list: vec![display(&[[1920, 1080]], [1920, 1080])],
        primary: 0,
        windowed_size: [1366, 768],
    };
    sender.send(displays.clone()).expect("thread listens");
    sender.send(displays.clone()).expect("thread listens");
    let mut second = displays;
    second.list.push(display(&[[1280, 720]], [1280, 720]));
    sender.send(second).expect("thread listens");
    let mut received = Vec::new();
    for _ in 0..1000 {
        while let Ok(lists) = lists_link.lists.pop() {
            received.push(lists);
        }
        if received.len() >= 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        received
            .iter()
            .map(|l| (l.displays.len(), l.generation))
            .collect::<Vec<_>>(),
        vec![(1, 1), (2, 2)]
    );
    assert!(views_link.take().is_none());
    for lists in received {
        lists_link.give_back(lists);
    }
    drop(thread);
}

/// A settings screen's rows holding text views.
struct Screen(Vec<String>);

impl DropdownRows<String> for Screen {
    fn swap_dropdown(&mut self, row: usize, view: &mut String) -> bool {
        match self.0.get_mut(row) {
            Some(own) => {
                std::mem::swap(own, view);
                true
            }
            None => false,
        }
    }
}

const ROWS: [usize; 3] = [0, 1, 2];

fn screen() -> Screen {
    Screen(vec![
        "own display".into(),
        "own full".into(),
        "own windowed".into(),
    ])
}

fn views(generation: u64, displays: usize) -> Box<DisplayViews<String>> {
    Box::new(DisplayViews {
        generation,
        display: format!("g{generation} displays"),
        fullscreen: (0..displays)
            .map(|d| format!("g{generation} full {d}"))
            .collect(),
        windowed: (0..displays)
            .map(|d| format!("g{generation} windowed {d}"))
            .collect(),
    })
}

/// The `slam-displays` thread's ends of the view rings: views in, old views back.
type ThreadEnds = (
    rtrb::Producer<Box<DisplayViews<String>>>,
    rtrb::Consumer<Box<DisplayViews<String>>>,
);

/// A [`ShownViews`] and the thread's ends of its rings.
fn shown_views() -> (ShownViews<String>, ThreadEnds) {
    let (producer, views) = rtrb::RingBuffer::new(RING);
    let (old, returned) = rtrb::RingBuffer::new(RING);
    (
        ShownViews::new(ViewsLink { views, old }, ROWS),
        (producer, returned),
    )
}

#[test]
fn views_wait_for_the_update_thread_to_show_their_lists() {
    let (mut shown, (mut producer, _returned)) = shown_views();
    let mut screen = screen();
    producer.push(views(1, 2)).unwrap();
    shown.receive();
    // The update thread still shows no lists.
    shown.show(&mut screen, 0, 0);
    assert_eq!(screen.0, ["own display", "own full", "own windowed"]);
    shown.show(&mut screen, 1, 1);
    assert_eq!(screen.0, ["g1 displays", "g1 full 1", "g1 windowed 1"]);
    shown.show(&mut screen, 1, 0);
    assert_eq!(screen.0, ["g1 displays", "g1 full 0", "g1 windowed 0"]);
}

#[test]
fn new_views_give_the_screen_its_own_back_first() {
    let (mut shown, (mut producer, mut returned)) = shown_views();
    let mut screen = screen();
    producer.push(views(1, 2)).unwrap();
    shown.receive();
    shown.show(&mut screen, 1, 1);
    // Fewer displays now; the screen still has display 1 chosen for a frame.
    producer.push(views(2, 1)).unwrap();
    shown.receive();
    shown.show(&mut screen, 2, 1);
    assert_eq!(screen.0, ["g2 displays", "own full", "own windowed"]);
    shown.show(&mut screen, 2, 0);
    assert_eq!(screen.0, ["g2 displays", "g2 full 0", "g2 windowed 0"]);
    // The old views went back whole, with their own texts in place.
    let old = returned.pop().expect("the old views go back");
    assert_eq!(old.display, "g1 displays");
    assert_eq!(old.fullscreen, ["g1 full 0", "g1 full 1"]);
    assert_eq!(old.windowed, ["g1 windowed 0", "g1 windowed 1"]);
}

#[test]
fn views_older_than_the_lists_shown_are_taken_off() {
    let (mut shown, (mut producer, mut returned)) = shown_views();
    let mut screen = screen();
    producer.push(views(1, 1)).unwrap();
    shown.receive();
    shown.show(&mut screen, 1, 0);
    // The update thread took lists 3 (and skipped 2) before the views arrive.
    producer.push(views(2, 1)).unwrap();
    shown.receive();
    shown.show(&mut screen, 3, 0);
    assert_eq!(screen.0, ["own display", "own full", "own windowed"]);
    producer.push(views(3, 1)).unwrap();
    shown.receive();
    assert_eq!(returned.pop().ok().map(|v| v.generation), Some(2));
    shown.show(&mut screen, 3, 0);
    assert_eq!(screen.0, ["g3 displays", "g3 full 0", "g3 windowed 0"]);
    assert_eq!(returned.pop().ok().map(|v| v.generation), Some(1));
}

#[test]
fn a_screen_without_the_rows_keeps_the_views() {
    let (mut shown, (mut producer, _returned)) = shown_views();
    let mut screen = Screen(Vec::new());
    producer.push(views(1, 1)).unwrap();
    shown.receive();
    shown.show(&mut screen, 1, 0);
    shown.show(&mut screen, 1, 3);
    let views = shown.views.as_ref().expect("taken");
    assert_eq!(views.display, "g1 displays");
    assert_eq!(views.fullscreen, ["g1 full 0"]);
}

#[test]
fn a_windowed_size_fits_below_the_title_bar_as_in_lazer() {
    // A 40 px taskbar at the bottom and a 30 px title bar.
    let bounds = [0, 0, 1920, 1080];
    let usable = [0, 0, 1920, 1040];
    // Centred in the usable area.
    let (size, position) = fit_windowed([1280, 720], bounds, usable, 30.0);
    assert_eq!(size, [1280, 720]);
    assert_eq!(position, [0.5, 160.0 / 360.0]);
    // Too tall: cut to the usable height minus the title bar, which stays visible.
    let (size, position) = fit_windowed([1920, 1080], bounds, usable, 30.0);
    assert_eq!(size, [1920, 1010]);
    assert_eq!(position, [0.0, 30.0 / 70.0]);
    // A second display left of the first, with its panel on the left.
    let bounds = [-1280, 0, 1280, 1024];
    let usable = [-1230, 0, 1230, 1024];
    let (size, position) = fit_windowed([1024, 768], bounds, usable, 0.0);
    assert_eq!(size, [1024, 768]);
    assert_eq!(position, [(50.0 + 103.0) / 256.0, 0.5]);
}

#[test]
fn a_fitted_windowed_size_keeps_the_settings_limits() {
    // A tiny usable area still gives the smallest windowed size, as lazer's bindable.
    let (size, position) = fit_windowed([800, 600], [0, 0, 800, 600], [0, 0, 600, 400], 30.0);
    assert_eq!(size, [640, 480]);
    assert!(position.iter().all(|p| (-0.5..=1.5).contains(p)));
}

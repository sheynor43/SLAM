//! The song list's scrollbar (ADR-0174).

use super::SongSelectInput as I;
use super::*;

fn list(sets: &[(u64, usize)]) -> Arc<SongList> {
    Arc::new(SongList {
        sets: sets
            .iter()
            .map(|&(key, diffs)| SetRow {
                key,
                artist: format!("artist {key}"),
                title: format!("title {key}"),
                creator: "mapper".to_owned(),
                difficulties: (0..diffs)
                    .map(|d| {
                        DifficultyRow::new(SearchSource {
                            difficulty: &format!("diff {d}"),
                            ..SearchSource::default()
                        })
                    })
                    .collect(),
            })
            .collect(),
    })
}

fn screen(sets: &[(u64, usize)]) -> SongSelect {
    let mut s = SongSelect::new();
    let list = list(sets);
    let filter = SongFilter::for_list(&list);
    s.set_list(list, filter);
    s
}

fn layout() -> SongSelectLayout {
    SongSelectLayout::new(1280.0, 720.0, &Theme::default())
}

fn press(s: &mut SongSelect, input: SongSelectInput) -> Option<SongSelectAction> {
    s.input(input, &layout())
}

#[test]
fn the_scrollbar_sits_at_the_windows_right_edge_between_lazers_paddings() {
    let theme = Theme::default();
    let l = layout();
    assert_eq!(l.scrollbar.x + l.scrollbar.width, l.window_right);
    assert_eq!(l.scrollbar.width, theme.song_scrollbar_width * l.scale);
    assert_eq!(
        l.scrollbar.y,
        l.list.y + theme.song_scrollbar_pad_top * l.scale
    );
    let bottom = l.scrollbar.y + l.scrollbar.height;
    assert!((720.0 - bottom - theme.song_scrollbar_pad_bottom * l.scale).abs() < 1e-3);
}

#[test]
fn a_short_list_has_no_scrollbar() {
    let mut s = screen(&[(1, 2), (2, 2)]);
    let l = layout();
    assert!(!s.scrollbar(&l).is_scrollable());
    // A press there is not the bar's.
    let (x, y) = (l.scrollbar.x + 1.0, l.scrollbar.y + 40.0);
    press(&mut s, I::PointerPress { x, y });
    assert!(!s.snapshot().scrollbar_held);
}

#[test]
fn a_press_on_the_track_jumps_and_a_drag_scrolls_the_list_at_once() {
    let sets: Vec<(u64, usize)> = (0..100).map(|k| (k, 2)).collect();
    let mut s = screen(&sets);
    let l = layout();
    s.tick(1, &l);
    assert!(s.scrollbar(&l).is_scrollable());
    let x = l.scrollbar.x + l.scrollbar.width / 2.0;
    // Low on the track: far from the thumb at the top.
    let y = l.scrollbar.y + l.scrollbar.height * 0.75;
    assert_eq!(press(&mut s, I::PointerPress { x, y }), None);
    let snap = s.snapshot();
    assert!(snap.scrollbar_held);
    assert!(snap.scroll > 0.0);
    // No easing: what is shown is where the bar put it, and the press selected nothing.
    assert_eq!(snap.shown_scroll, snap.scroll);
    assert_eq!(snap.selected, Some((0, 0)));
    let thumb = s.scrollbar(&l).thumb(snap.shown_scroll);
    assert!((thumb.y + thumb.height / 2.0 - y).abs() < 1.0);
    // Dragging up moves the list back.
    let jumped = snap.scroll;
    press(&mut s, I::PointerMove { x, y: y - 100.0 });
    assert!(s.snapshot().scroll < jumped);
    // To the end of the track the list ends.
    press(&mut s, I::PointerMove { x, y: 10_000.0 });
    let end = s.snapshot();
    assert_eq!(end.scroll, s.max_scroll());
    // The release lets go and clicks nothing.
    assert_eq!(press(&mut s, I::PointerRelease), None);
    assert!(!s.snapshot().scrollbar_held);
    assert_eq!(s.snapshot().selected, Some((0, 0)));
    // Moves no longer scroll.
    press(&mut s, I::PointerMove { x, y: y - 100.0 });
    assert_eq!(s.snapshot().scroll, end.scroll);
}

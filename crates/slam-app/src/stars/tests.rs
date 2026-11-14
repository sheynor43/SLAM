use std::fs;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

use super::*;

fn index(keys: &[u64]) -> HashMap<u64, usize> {
    keys.iter().enumerate().map(|(i, k)| (*k, i)).collect()
}

#[test]
fn visible_sets_come_first_then_list_order() {
    let keys = [10, 11, 12, 13, 14, 15, 16, 17, 18, 19];
    let mut done = vec![false; keys.len()];
    let index = index(&keys);
    let mut cursor = 0;
    let mut order = Vec::new();
    // The player looks at set 16 with room for two sets; the margin adds four.
    while let Some(set) = next_set(&done, &index, (16, 2), &mut cursor) {
        done[set] = true;
        order.push(set);
    }
    assert_eq!(order, [6, 7, 8, 9, 0, 1, 2, 3, 4, 5]);
}

#[test]
fn a_focus_on_an_unknown_set_works_in_list_order() {
    let mut done = vec![false; 3];
    let mut cursor = 0;
    assert_eq!(
        next_set(&done, &index(&[1, 2, 3]), (99, 5), &mut cursor),
        Some(0)
    );
    done[0] = true;
    done[1] = true;
    done[2] = true;
    assert_eq!(
        next_set(&done, &index(&[1, 2, 3]), (99, 5), &mut cursor),
        None
    );
}

#[test]
fn focus_moving_mid_run_changes_the_next_set() {
    let done = vec![
        true, false, false, false, false, false, false, false, false, false,
    ];
    let keys: Vec<u64> = (0..10).collect();
    let mut cursor = 0;
    assert_eq!(next_set(&done, &index(&keys), (0, 1), &mut cursor), Some(1));
    assert_eq!(next_set(&done, &index(&keys), (8, 1), &mut cursor), Some(8));
}

struct Tmp(PathBuf);

impl Tmp {
    fn new(name: &str) -> Tmp {
        let dir = std::env::temp_dir().join(format!("slam-stars-w-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Tmp(dir)
    }

    fn file(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, name).unwrap();
        path
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn collect(link: &mut StarsLink, want: usize) -> HashMap<PathBuf, f32> {
    let start = Instant::now();
    let mut got = HashMap::new();
    while got.len() < want {
        assert!(start.elapsed() < Duration::from_secs(10), "timed out");
        link.poll(|path, stars| {
            got.insert(path, stars);
        });
        std::thread::sleep(Duration::from_millis(2));
    }
    got
}

#[test]
fn ratings_arrive_and_the_cache_spares_the_second_run() {
    let tmp = Tmp::new("cache");
    let files: Vec<PathBuf> = (0..4).map(|i| tmp.file(&format!("{i}.osu"))).collect();
    let cache = tmp.0.join("cache").join("stars.bin");
    let calls = Arc::new(AtomicUsize::new(0));
    let sets = |files: &[PathBuf]| {
        vec![
            StarSet {
                key: 1,
                files: files[..2].to_vec(),
            },
            StarSet {
                key: 2,
                files: files[2..].to_vec(),
            },
        ]
    };
    let make = |calls: &Arc<AtomicUsize>| -> Calculator {
        let calls = Arc::clone(calls);
        Box::new(move |path| {
            calls.fetch_add(1, Ordering::Relaxed);
            let digit = path
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .parse::<f32>()
                .unwrap();
            Ok(digit + 0.5)
        })
    };
    let mut link =
        StarsLink::spawn(Some(cache.clone()), make(&calls), "t1", Arc::default()).unwrap();
    link.start(sets(&files));
    let got = collect(&mut link, 4);
    assert_eq!(got[&files[3]], 3.5);
    assert_eq!(calls.load(Ordering::Relaxed), 4);
    drop(link);
    assert!(cache.exists());

    // A second run reads the cache.
    let mut link =
        StarsLink::spawn(Some(cache.clone()), make(&calls), "t1", Arc::default()).unwrap();
    link.start(sets(&files));
    assert_eq!(collect(&mut link, 4)[&files[0]], 0.5);
    assert_eq!(calls.load(Ordering::Relaxed), 4);
    drop(link);

    // Another calculator version recalculates everything.
    let mut link = StarsLink::spawn(Some(cache), make(&calls), "t2", Arc::default()).unwrap();
    link.start(sets(&files));
    collect(&mut link, 4);
    assert_eq!(calls.load(Ordering::Relaxed), 8);
}

#[test]
fn unreadable_files_are_skipped_and_a_new_run_ignores_old_results() {
    let tmp = Tmp::new("skip");
    let good = tmp.file("1.osu");
    let missing = tmp.0.join("missing.osu");
    let calc: Calculator = Box::new(|path| {
        if path.exists() {
            Ok(2.0)
        } else {
            Err(io::Error::other("gone"))
        }
    });
    let mut link = StarsLink::spawn(None, calc, "t", Arc::default()).unwrap();
    link.start(vec![StarSet {
        key: 1,
        files: vec![missing, good.clone()],
    }]);
    let got = collect(&mut link, 1);
    assert_eq!(got.len(), 1);
    assert_eq!(got[&good], 2.0);
    // Results of the first run that arrive after the second started are dropped.
    link.start(Vec::new());
    std::thread::sleep(Duration::from_millis(50));
    assert!(!link.poll(|_, _| panic!("stale rating")));
}

#[test]
fn a_panicking_calculator_neither_kills_the_worker_nor_is_asked_again() {
    let tmp = Tmp::new("panic");
    let bad = tmp.file("bad.osu");
    let good = tmp.file("good.osu");
    let cache = tmp.0.join("stars.bin");
    let calls = Arc::new(AtomicUsize::new(0));
    let make = |calls: &Arc<AtomicUsize>| -> Calculator {
        let calls = Arc::clone(calls);
        Box::new(move |path| {
            calls.fetch_add(1, Ordering::Relaxed);
            if path.ends_with("bad.osu") {
                panic!("rosu-pp blew up");
            }
            Ok(4.0)
        })
    };
    let sets = || {
        vec![StarSet {
            key: 1,
            files: vec![bad.clone(), good.clone()],
        }]
    };
    let mut link =
        StarsLink::spawn(Some(cache.clone()), make(&calls), "t", Arc::default()).unwrap();
    link.start(sets());
    let got = collect(&mut link, 1);
    assert_eq!(got[&good], 4.0);
    assert!(!got.contains_key(&bad));
    // Alive: another run still works.
    link.start(sets());
    collect(&mut link, 1);
    drop(link);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    // The failure is in the cache: the next start does not calculate again.
    let mut link = StarsLink::spawn(Some(cache), make(&calls), "t", Arc::default()).unwrap();
    link.start(sets());
    assert_eq!(collect(&mut link, 1)[&good], 4.0);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[test]
fn a_play_pauses_the_worker_until_it_ends() {
    let tmp = Tmp::new("pause");
    let file = tmp.file("1.osu");
    let focus = Arc::<StarFocus>::default();
    focus.set_paused(true);
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let calc: Calculator = Box::new(move |_| {
        counter.fetch_add(1, Ordering::Relaxed);
        Ok(1.0)
    });
    let mut link = StarsLink::spawn(None, calc, "t", Arc::clone(&focus)).unwrap();
    link.start(vec![StarSet {
        key: 1,
        files: vec![file.clone()],
    }]);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(!link.poll(|_, _| panic!("rated during a play")));
    focus.set_paused(false);
    link.wake();
    assert_eq!(collect(&mut link, 1)[&file], 1.0);
}

#[test]
fn cancel_stops_the_run() {
    let tmp = Tmp::new("cancel");
    let files: Vec<PathBuf> = (0..50).map(|i| tmp.file(&format!("{i}.osu"))).collect();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let calc: Calculator = Box::new(move |_| {
        counter.fetch_add(1, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(20));
        Ok(1.0)
    });
    let mut link = StarsLink::spawn(None, calc, "t", Arc::default()).unwrap();
    link.start(
        files
            .iter()
            .enumerate()
            .map(|(i, f)| StarSet {
                key: i as u64,
                files: vec![f.clone()],
            })
            .collect(),
    );
    std::thread::sleep(Duration::from_millis(100));
    link.cancel();
    std::thread::sleep(Duration::from_millis(100));
    let after = calls.load(Ordering::Relaxed);
    assert!(after < 50);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(calls.load(Ordering::Relaxed), after);
    assert!(!link.poll(|_, _| panic!("stale rating")));
}

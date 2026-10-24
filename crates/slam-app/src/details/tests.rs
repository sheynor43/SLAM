use std::path::PathBuf;

use super::*;
use crate::songs::SongFile;

const MS: u64 = 1_000_000;

fn file(path: PathBuf) -> SongFile {
    SongFile {
        path,
        approach_rate: 9.0,
        circle_size: 4.0,
        drain_rate: 5.0,
        overall_difficulty: 8.0,
        background: None,
    }
}

/// Set 0: two difficulties.
fn files(dir: &Path) -> Arc<SongFiles> {
    Arc::from(vec![
        vec![file(dir.join("a.osu")), file(dir.join("b.osu"))].into_boxed_slice(),
    ])
}

fn statistics(circles: u32) -> BeatmapStatistics {
    BeatmapStatistics {
        length: 1000.0,
        drain_length: 1000.0,
        bpm_minimum: 120.0,
        bpm_maximum: 120.0,
        bpm_most_common: 120.0,
        circles,
        sliders: 0,
        spinners: 0,
    }
}

struct Rig {
    picker: DetailsPicker,
    requests: rtrb::Consumer<DetailsRequest>,
    replies: rtrb::Producer<DetailsReply>,
    files: Arc<SongFiles>,
}

impl Rig {
    fn new() -> Self {
        let (picker, reader) = link();
        let DetailsReader {
            requests, replies, ..
        } = reader;
        Self {
            picker,
            requests,
            replies,
            files: files(Path::new("/set")),
        }
    }

    fn tick(&mut self, now_ns: u64, selected: Option<(usize, usize)>) -> Option<u32> {
        self.picker
            .tick(now_ns, &self.files, selected)
            .map(|s| s.circles)
    }

    fn asked(&mut self) -> Vec<(u32, usize, usize)> {
        let mut asked = Vec::new();
        while let Ok(r) = self.requests.pop() {
            asked.push((r.generation, r.set, r.difficulty));
        }
        asked
    }

    fn reply(&mut self, generation: u32, circles: u32) {
        let reply = DetailsReply {
            generation,
            statistics: Some(statistics(circles)),
        };
        self.replies.push(reply).unwrap();
    }
}

#[test]
fn asks_once_the_selection_settles_and_shows_the_answer() {
    let mut rig = Rig::new();
    rig.tick(0, Some((0, 0)));
    assert_eq!(rig.tick(149 * MS, Some((0, 0))), None);
    assert_eq!(rig.asked(), []);
    rig.tick(150 * MS, Some((0, 0)));
    assert_eq!(rig.asked(), [(1, 0, 0)]);
    rig.reply(1, 7);
    assert_eq!(rig.tick(160 * MS, Some((0, 0))), Some(7));
    assert_eq!(rig.asked(), [], "asked once");
}

#[test]
fn another_difficulty_hides_the_answer_until_its_own_comes() {
    let mut rig = Rig::new();
    rig.tick(0, Some((0, 0)));
    rig.tick(150 * MS, Some((0, 0)));
    rig.reply(1, 7);
    assert_eq!(rig.tick(160 * MS, Some((0, 0))), Some(7));
    assert_eq!(rig.tick(170 * MS, Some((0, 1))), None);
    rig.tick(320 * MS, Some((0, 1)));
    assert_eq!(rig.asked(), [(1, 0, 0), (2, 0, 1)]);
    // A late answer to the first request is dropped.
    rig.reply(1, 7);
    assert_eq!(rig.tick(330 * MS, Some((0, 1))), None);
    rig.reply(2, 9);
    assert_eq!(rig.tick(340 * MS, Some((0, 1))), Some(9));
}

#[test]
fn nothing_is_asked_without_a_selection() {
    let mut rig = Rig::new();
    rig.tick(0, None);
    assert_eq!(rig.tick(500 * MS, None), None);
    assert_eq!(rig.asked(), []);
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_reader_answers_the_newest_request_from_the_file() {
    let t = TempDir(std::env::temp_dir().join(format!("slam-details-{}", std::process::id())));
    std::fs::create_dir_all(&t.0).unwrap();
    std::fs::write(
        t.0.join("a.osu"),
        "osu file format v14\n[General]\nMode: 0\n[TimingPoints]\n0,500,4,2,0,70,1,0\n[HitObjects]\n256,192,1000,1,0\n256,192,3000,1,0\n",
    )
    .unwrap();
    let files = files(&t.0);
    let (_picker, mut reader) = link();
    let (mut requests, mut replies) = {
        let (tx, rx) = rtrb::RingBuffer::new(RING);
        let (reply_tx, reply_rx) = rtrb::RingBuffer::new(RING);
        reader.requests = rx;
        reader.replies = reply_tx;
        (tx, reply_rx)
    };
    for (generation, difficulty) in [(1, 1), (2, 0)] {
        let request = DetailsRequest {
            generation,
            files: Arc::clone(&files),
            set: 0,
            difficulty,
        };
        assert!(requests.push(request).is_ok());
    }
    reader.poll();
    let reply = replies.pop().unwrap();
    assert_eq!(reply.generation, 2);
    let s = reply.statistics.unwrap();
    assert_eq!((s.length, s.circles, s.bpm_most_common), (2000.0, 2, 120.0));
    assert!(replies.pop().is_err(), "the older request is dropped");

    // `b.osu` does not exist.
    let request = DetailsRequest {
        generation: 3,
        files: Arc::clone(&files),
        set: 0,
        difficulty: 1,
    };
    assert!(requests.push(request).is_ok());
    reader.poll();
    assert_eq!(
        replies.pop().unwrap(),
        DetailsReply {
            generation: 3,
            statistics: None
        }
    );
    assert_eq!(Arc::strong_count(&files), 1);
}

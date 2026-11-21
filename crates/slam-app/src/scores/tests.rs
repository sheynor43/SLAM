use super::*;

const HASH: &str = "9e107d9d372bb6826d81d3542a419d6a";

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("slam-scores-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn record(total_score: i64) -> Record {
    Record {
        date: 1_794_681_000_000,
        total_score,
        accuracy: 0.9845,
        max_combo: 321,
        rank: "SH".into(),
        mods: vec!["HD".into(), "DT".into()],
        statistics: BTreeMap::from([("great".to_owned(), 300), ("miss".to_owned(), 2)]),
        maximum_statistics: BTreeMap::from([
            ("great".to_owned(), 302),
            ("max_combo".to_owned(), 400),
        ]),
        player: "Guest".into(),
        replay: Some("map (1).osr".into()),
    }
}

#[test]
fn records_round_trip_through_json() {
    let t = TempDir::new("round");
    append(&t.0, HASH, record(5)).unwrap();
    let text = std::fs::read_to_string(t.0.join(format!("{HASH}.json"))).unwrap();
    assert!(text.contains("\"version\": 1"), "{text}");
    assert!(text.contains("\"great\": 300"), "{text}");
    assert_eq!(load_top(&t.0, HASH, 10), [record(5)]);
    assert!(!t.0.join(format!("{HASH}.json.tmp")).exists());
}

#[test]
fn a_record_without_a_replay_keeps_null() {
    let t = TempDir::new("null");
    let mut r = record(5);
    r.replay = None;
    append(&t.0, HASH, r.clone()).unwrap();
    let text = std::fs::read_to_string(t.0.join(format!("{HASH}.json"))).unwrap();
    assert!(text.contains("\"replay\": null"), "{text}");
    assert_eq!(load_top(&t.0, HASH, 10), [r]);
}

#[test]
fn appending_keeps_the_old_records_and_the_top_is_best_first() {
    let t = TempDir::new("top");
    for score in [30, 10, 50, 20, 40] {
        append(&t.0, HASH, record(score)).unwrap();
    }
    let top: Vec<_> = load_top(&t.0, HASH, 3)
        .iter()
        .map(|r| r.total_score)
        .collect();
    assert_eq!(top, [50, 40, 30]);
    assert_eq!(load_top(&t.0, HASH, 10).len(), 5);
}

#[test]
fn a_missing_file_has_no_scores() {
    let t = TempDir::new("missing");
    assert!(load_top(&t.0, HASH, 10).is_empty());
}

#[test]
fn an_unreadable_file_is_kept_aside_and_a_new_one_started() {
    let t = TempDir::new("bad");
    std::fs::create_dir_all(&t.0).unwrap();
    let path = t.0.join(format!("{HASH}.json"));
    std::fs::write(&path, "{ not json").unwrap();
    assert!(load_top(&t.0, HASH, 10).is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    append(&t.0, HASH, record(7)).unwrap();
    assert_eq!(
        std::fs::read_to_string(t.0.join(format!("{HASH}.json.bad"))).unwrap(),
        "{ not json"
    );
    assert_eq!(load_top(&t.0, HASH, 10), [record(7)]);
}

#[test]
fn a_file_of_another_version_is_kept_aside() {
    let t = TempDir::new("version");
    std::fs::create_dir_all(&t.0).unwrap();
    let path = t.0.join(format!("{HASH}.json"));
    std::fs::write(&path, r#"{"version": 2, "scores": []}"#).unwrap();
    assert!(load_top(&t.0, HASH, 10).is_empty());
    append(&t.0, HASH, record(7)).unwrap();
    assert!(t.0.join(format!("{HASH}.json.bad")).exists());
    assert_eq!(load_top(&t.0, HASH, 10).len(), 1);
}

#[test]
fn only_a_lowercase_md5_names_a_file() {
    assert!(valid_hash(HASH));
    for bad in [
        "",
        "../../etc/passwd",
        "9E107D9D372BB6826D81D3542A419D6A",
        "9e107d9d372bb6826d81d3542a419d6",
        "9e107d9d372bb6826d81d3542a419d6ab",
        "9e107d9d372bb6826d81d3542a419d6/",
    ] {
        assert!(!valid_hash(bad), "{bad}");
    }
    let t = TempDir::new("hash");
    assert!(append(&t.0, "../escape", record(1)).is_err());
    assert!(!t.0.exists() || std::fs::read_dir(&t.0).unwrap().next().is_none());
    assert!(load_top(&t.0, "../escape", 10).is_empty());
}

#[test]
fn rows_carry_the_top_scores_into_the_screens_type() {
    let records = [record(900), record(800)];
    let scores = rows_of(&records);
    let first = scores.rows[0].unwrap();
    assert_eq!(first.total_score, 900);
    assert_eq!(first.rank, *b"SH");
    assert_eq!(first.max_combo, 321);
    assert_eq!(first.mods.iter().collect::<Vec<_>>(), ["HD", "DT"]);
    assert!(scores.rows[2].is_none());
    let mut single = record(1);
    single.rank = "A".into();
    assert_eq!(rows_of(&[single]).rows[0].unwrap().rank, [b'A', 0]);
}

#[test]
fn only_passed_ranks_are_kept() {
    for rank in [
        ScoreRank::XH,
        ScoreRank::X,
        ScoreRank::SH,
        ScoreRank::S,
        ScoreRank::A,
        ScoreRank::B,
        ScoreRank::C,
        ScoreRank::D,
    ] {
        assert!(is_kept(rank), "{rank:?}");
    }
    assert!(!is_kept(ScoreRank::F));
}

#[test]
fn autoplay_plays_are_not_kept_but_relax_and_autopilot_ones_are() {
    use slam_osu::{Mod, ModSet};
    let set = |acronyms: &[&str]| {
        ModSet::new(acronyms.iter().map(|a| Mod::from_acronym(a)).collect()).unwrap()
    };
    assert!(is_kept_play(&set(&[])));
    assert!(!is_kept_play(&set(&["AT"])));
    assert!(!is_kept_play(&set(&["HD", "AT"])));
    assert!(is_kept_play(&set(&["RX"])));
    assert!(is_kept_play(&set(&["AP"])));
}

#[test]
fn rows_carry_what_the_results_of_a_score_show() {
    let rows = rows_of(&[record(5)]);
    let row = rows.rows[0].unwrap();
    assert_eq!(row.hits[StatKind::Great.index()], 300);
    assert_eq!(row.hits[StatKind::Miss.index()], 2);
    assert_eq!(row.maximums[StatKind::Great.index()], 302);
    assert_eq!(row.maximum_combo, 400);
    assert_eq!(row.player.as_str(), "Guest");
}

#[test]
fn a_record_saved_without_the_perfect_counts_still_reads() {
    let json = r#"{"date":1,"total_score":5,"accuracy":0.5,"max_combo":3,"rank":"A",
        "mods":[],"statistics":{"great":3},"player":"Guest","replay":null}"#;
    let record: Record = serde_json::from_str(json).unwrap();
    assert!(record.maximum_statistics.is_empty());
    let row = rows_of(&[record]).rows[0].unwrap();
    assert_eq!((row.maximum_combo, row.maximums), (0, [0; StatKind::COUNT]));
}

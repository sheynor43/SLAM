//! Triple buffer under concurrent use: the reader never sees a torn or older snapshot.

use slam_engine::triple_buffer;

const LEN: usize = 64;
const PUBLISHES: u64 = 200_000;

#[test]
fn reader_sees_whole_snapshots_in_order() {
    let (mut writer, mut reader) = triple_buffer([0u64; LEN]);
    let producer = std::thread::spawn(move || {
        for v in 1..=PUBLISHES {
            writer.slot().fill(v);
            writer.publish();
        }
    });
    let mut last = 0;
    let mut fresh_reads = 0u64;
    while last < PUBLISHES {
        let (snapshot, fresh) = reader.read();
        let v = snapshot[0];
        assert!(snapshot.iter().all(|&x| x == v), "torn snapshot");
        if fresh {
            assert!(v > last, "fresh snapshot {v} not newer than {last}");
            fresh_reads += 1;
        } else {
            assert_eq!(v, last, "stale read changed value");
        }
        last = v;
    }
    producer.join().unwrap();
    assert!(fresh_reads > 0);
}

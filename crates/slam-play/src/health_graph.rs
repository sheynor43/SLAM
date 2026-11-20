//! The health of a play over its playable time, for the results' graph (ADR-0161).

/// Points of the curve: one per this share of the playable time.
pub const HEALTH_SAMPLES: usize = 256;

/// Health over the playable time, sampled into fixed buckets as the play goes: the last
/// value a bucket saw. Fixed size, so recording never allocates.
#[derive(Debug, Clone, PartialEq)]
pub struct HealthGraph {
    start: f64,
    end: f64,
    /// NaN for a bucket nothing fell in.
    samples: [f32; HEALTH_SAMPLES],
}

impl HealthGraph {
    /// A graph from the first object's start to the last object's end, game ms.
    pub fn new(start: f64, end: f64) -> Self {
        HealthGraph {
            start,
            end,
            samples: [f32::NAN; HEALTH_SAMPLES],
        }
    }

    /// Takes `health` (0..=1) at game time `time`. Before the first object it is not drawn;
    /// past the last object's end it goes to the last point, which so ends with the
    /// judgements that come after (a late hit or miss of the last circle).
    pub fn record(&mut self, time: f64, health: f64) {
        let span = self.end - self.start;
        if span <= 0.0 || span.is_nan() || time < self.start {
            return;
        }
        let index = (((time - self.start) / span) * HEALTH_SAMPLES as f64) as usize;
        self.samples[index.min(HEALTH_SAMPLES - 1)] = health.clamp(0.0, 1.0) as f32;
    }

    /// Forgets the attempt.
    pub fn restart(&mut self) {
        self.samples = [f32::NAN; HEALTH_SAMPLES];
    }

    /// The curve, [`HEALTH_SAMPLES`] points from the start to the end: a bucket nothing
    /// fell in has the one before it (full health before the first). Allocates.
    pub fn curve(&self) -> Vec<f32> {
        let mut last = 1.0;
        self.samples
            .iter()
            .map(|&s| {
                if !s.is_nan() {
                    last = s;
                }
                last
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_keep_their_last_health_and_gaps_the_one_before() {
        let mut graph = HealthGraph::new(1000.0, 1256.0);
        graph.record(500.0, 0.1);
        graph.record(1000.0, 0.9);
        graph.record(1000.5, 0.8);
        graph.record(1010.0, 0.5);
        graph.record(1256.0, 0.3);
        let curve = graph.curve();
        assert_eq!(curve.len(), HEALTH_SAMPLES);
        assert_eq!(curve[0], 0.8);
        assert_eq!(curve[5], 0.8);
        assert_eq!(curve[10], 0.5);
        assert_eq!(curve[254], 0.5);
        assert_eq!(curve[255], 0.3);
        // A judgement after the last object's end lands on the last point.
        graph.record(2000.0, 0.25);
        assert_eq!(graph.curve()[255], 0.25);
        graph.restart();
        assert!(graph.curve().iter().all(|&h| h == 1.0));
    }

    #[test]
    fn an_empty_map_draws_nothing() {
        let mut graph = HealthGraph::new(0.0, 0.0);
        graph.record(0.0, 0.5);
        assert!(graph.curve().iter().all(|&h| h == 1.0));
    }
}

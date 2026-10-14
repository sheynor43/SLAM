//! Building slider body meshes and gradients: done once per slider at load.
//!
//! Run: `cargo bench -p slam-render --bench slider_mesh`. Not built for Windows
//! targets (no criterion there, see `Cargo.toml`).

#[cfg(windows)]
fn main() {}

#[cfg(not(windows))]
mod bench {
    use std::hint::black_box;

    use criterion::{Criterion, criterion_group};
    use slam_render::slider::{SliderGradient, SliderMesh};

    /// A wavy path of `points` points about 2 osu! pixels apart, like a calculated
    /// Bézier path.
    fn path(points: usize) -> (Vec<[f32; 2]>, Vec<f64>) {
        let mut pts = Vec::with_capacity(points);
        let mut cumulative = Vec::with_capacity(points);
        let mut length = 0.0f64;
        for i in 0..points {
            let t = i as f32 * 2.0;
            let p = [t, (t / 20.0).sin() * 40.0];
            if let Some(q) = pts.last() {
                let q: &[f32; 2] = q;
                length += f64::from(((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt());
            }
            pts.push(p);
            cumulative.push(length);
        }
        (pts, cumulative)
    }

    fn slider_mesh(c: &mut Criterion) {
        for points in [50, 200, 2000] {
            let (pts, cumulative) = path(points);
            c.bench_function(&format!("slider_mesh/{points}_points"), |b| {
                b.iter(|| SliderMesh::new(black_box(&pts), black_box(&cumulative)))
            });
        }
        c.bench_function("slider_gradient/radius_36", |b| {
            b.iter(|| SliderGradient::new(black_box(36.48), |p| [p, 0.5 * p, 1.0 - p, 0.7]))
        });
    }

    criterion_group!(benches, slider_mesh);
}

#[cfg(not(windows))]
criterion::criterion_main!(bench::benches);

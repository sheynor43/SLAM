//! MSDF generation time for a Latin + Cyrillic glyph set.
//!
//! Run: `cargo bench -p slam-text --bench msdf`. Not built for Windows targets (no
//! criterion there, see `Cargo.toml`).

#[cfg(windows)]
fn main() {}

#[cfg(not(windows))]
mod bench {
    use criterion::{Criterion, criterion_group};
    use skrifa::{FontRef, MetadataProvider};
    use slam_text::{FontRegistry, GlyphKey, HEADING_FAMILY, UI_FAMILY, generate_glyph};

    fn keys(registry: &FontRegistry, family: &str, weight: u16) -> Vec<GlyphKey> {
        let face = registry
            .resolve(registry.family(family).unwrap(), weight)
            .unwrap();
        let (data, index) = registry.face_data(face.font).unwrap();
        let font = FontRef::from_index(data, index).unwrap();
        let charmap = font.charmap();
        ('\u{21}'..='\u{7E}')
            .chain('А'..='я')
            .chain(['Ё', 'ё'])
            .filter_map(|c| charmap.map(c))
            .map(|gid| GlyphKey {
                font: face.font,
                weight: face.weight,
                glyph_id: gid.to_u32() as u16,
            })
            .collect()
    }

    fn bench(c: &mut Criterion) {
        let registry = FontRegistry::with_builtin();
        let mut group = c.benchmark_group("msdf");
        group.sample_size(10);
        for (name, family, weight) in [
            ("nunito_sans_400", UI_FAMILY, 400),
            ("exo2_700", HEADING_FAMILY, 700),
        ] {
            let set = keys(&registry, family, weight);
            println!("{name}: {} glyphs", set.len());
            group.bench_function(name, |b| {
                b.iter(|| {
                    for &key in &set {
                        std::hint::black_box(generate_glyph(&registry, key).unwrap());
                    }
                });
            });
        }
        group.finish();
    }

    criterion_group!(benches, bench);
}

// What `criterion_main!` expands to; the macro cannot sit inside the module.
#[cfg(not(windows))]
fn main() {
    bench::benches();
    criterion::Criterion::default()
        .configure_from_args()
        .final_summary();
}

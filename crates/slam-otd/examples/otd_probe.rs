//! Reads tablets and settings from the running OTD daemon (read-only).

use slam_otd::{Command, Event, short_type_name, worker::execute};

fn main() {
    match execute(&mut slam_otd::transport::connect, Command::Refresh) {
        Event::Snapshot(s) => {
            println!("OTD {} ({})", s.app.version, s.app.settings_file.display());
            for t in &s.tablets {
                println!("tablet: {} {}x{} mm", t.name, t.width_mm, t.height_mm);
            }
            for i in 0..s.settings.profile_count() {
                let Some(p) = s.settings.profile(i) else {
                    continue;
                };
                println!("profile {}: {}", p.tablet, short_type_name(&p.output_mode));
                if let Some(a) = p.absolute {
                    println!(
                        "  tablet area {:?}\n  display area {:?}",
                        a.tablet, a.display
                    );
                }
                for f in &p.filters {
                    println!(
                        "  filter {} enabled={}",
                        short_type_name(&f.path),
                        f.enabled
                    );
                }
            }
        }
        other => println!("{other:?}"),
    }
}

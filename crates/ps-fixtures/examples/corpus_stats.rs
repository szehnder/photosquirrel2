//! Generate a corpus into a folder and print statistics: `cargo run --release -p ps-fixtures --example
//! corpus_stats -- <preset> <seed> <out-dir>`.

use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let preset =
        ps_fixtures::Preset::parse(args.get(1).map_or("small", String::as_str)).expect("preset");
    let seed: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(42);
    let out = args.get(3).cloned().unwrap_or_else(|| "corpus-out".into());
    let w = ps_fixtures::DirWriter::new(&out).expect("output folder");
    let t = Instant::now();
    let truth = ps_fixtures::generate(seed, preset, &w).expect("generate");
    let secs = t.elapsed().as_secs_f64();
    println!(
        "{} files, {:.1} MB, {} groups, {} must-not-merge sets in {secs:.2} s",
        truth.files.len(),
        truth.total_bytes() as f64 / 1e6,
        truth.groups().len(),
        truth.must_not_merge.len()
    );
    println!("categories: {:?}", truth.category_counts());
    println!("classes: {:?}", truth.class_counts());
}

//! Task 4.6 of `heic-system-decoders`: the AVIF analysis decode time of a 12 MP AVIF, with and without rav1d's
//! assembly (the programs' `avif-asm` feature). A measurement, not a budget (a budget would need an ADR); run by
//! hand: `AVIF_12MP=<file.avif> cargo test --release -p photosquirrel-cli --test avif_timing -- --ignored
//! --nocapture` (add `--no-default-features` for the build without assembly).

#[test]
#[ignore = "measurement"]
fn avif_12mp_analysis_decode_time() {
    let Some(path) = std::env::var_os("AVIF_12MP") else {
        eprintln!("AVIF_12MP not set");
        return;
    };
    let bytes = std::fs::read(path).expect("read");
    let probe = ps_media::probe_bytes(&bytes, bytes.len() as u64, None);
    let decoders = ps_media::Decoders::none();
    let mut times = Vec::new();
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let d = ps_media::decode_for_analysis(&bytes, &probe, &decoders).expect("decode");
        times.push(t.elapsed());
        assert!(d.image.width >= 4000);
    }
    times.sort();
    eprintln!(
        "12 MP AVIF analysis decode (assembly {}): median {:?}, best {:?}",
        cfg!(all(feature = "avif-asm", target_arch = "x86_64")),
        times[2],
        times[0]
    );
}

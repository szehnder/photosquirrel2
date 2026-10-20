//! Minimal ISO-BMFF / QuickTime writer: `ftyp`, `moov` (`mvhd`, one video `trak` with `tkhd`, `mdia/mdhd`,
//! `hdlr`, `minf/stbl/stsd(avc1)`), `udta` with `©mak`, `©mod`, `©day`, `©xyz`, then an opaque `mdat`.
//! Enough for metadata probing and classification; frames are not decodable.

use crate::rng::mix64;

/// Seconds between 1904-01-01 (QuickTime epoch) and 1970-01-01.
const QT_EPOCH_OFFSET: i64 = 2_082_844_800;

#[derive(Clone, Debug)]
pub struct VideoSpec {
    /// `true`: QuickTime `.MOV` (`qt  ` brand); `false`: MP4 (`isom`/`mp42`).
    pub quicktime: bool,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    /// UTC creation time, Unix seconds.
    pub created_utc: i64,
    pub make: Option<String>,
    pub model: Option<String>,
    /// ISO 6709 location (`+37.7749-122.4194/`).
    pub location: Option<String>,
    /// Apple ContentIdentifier (`com.apple.quicktime.content.identifier`) of a Live Photo video.
    pub content_identifier: Option<String>,
    /// `mdat` payload size in bytes.
    pub payload: usize,
    pub seed: u64,
}

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(
        &u32::try_from(body.len() + 8)
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}

fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(body.len() + 4);
    b.push(version);
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(kind, &b)
}

fn qt_string(kind: &[u8; 4], s: &str) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&u16::try_from(s.len()).unwrap_or(u16::MAX).to_be_bytes());
    b.extend_from_slice(&0x15C7u16.to_be_bytes()); // language: en
    b.extend_from_slice(s.as_bytes());
    bx(kind, &b)
}

fn meta_box(items: &[(&str, String)]) -> Vec<u8> {
    let mut hdlr = vec![0u8; 4];
    hdlr.extend_from_slice(b"mdta");
    hdlr.extend_from_slice(&[0u8; 12]);
    hdlr.push(0);
    let hdlr = full(b"hdlr", 0, 0, &hdlr);
    let mut keys = u32::try_from(items.len())
        .unwrap_or(0)
        .to_be_bytes()
        .to_vec();
    for (k, _) in items {
        keys.extend_from_slice(&u32::try_from(k.len() + 8).unwrap_or(0).to_be_bytes());
        keys.extend_from_slice(b"mdta");
        keys.extend_from_slice(k.as_bytes());
    }
    let keys = full(b"keys", 0, 0, &keys);
    let mut ilst = Vec::new();
    for (i, (_, v)) in items.iter().enumerate() {
        let data_len = u32::try_from(v.len() + 16).unwrap_or(0);
        ilst.extend_from_slice(&(data_len + 8).to_be_bytes());
        ilst.extend_from_slice(&u32::try_from(i + 1).unwrap_or(0).to_be_bytes());
        ilst.extend_from_slice(&data_len.to_be_bytes());
        ilst.extend_from_slice(b"data");
        ilst.extend_from_slice(&1u32.to_be_bytes()); // type set 0, UTF-8
        ilst.extend_from_slice(&0u32.to_be_bytes()); // locale
        ilst.extend_from_slice(v.as_bytes());
    }
    bx(b"meta", &[hdlr, keys, bx(b"ilst", &ilst)].concat())
}

const MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

/// Serialise the container.
pub fn write_video(v: &VideoSpec) -> Vec<u8> {
    let timescale: u32 = 600;
    let duration = u32::try_from(v.duration_ms * u64::from(timescale) / 1000).unwrap_or(u32::MAX);
    let qt_time = u32::try_from((v.created_utc + QT_EPOCH_OFFSET).max(0)).unwrap_or(u32::MAX);

    let ftyp = if v.quicktime {
        bx(b"ftyp", b"qt  \x00\x00\x02\x00qt  ")
    } else {
        bx(b"ftyp", b"mp42\x00\x00\x00\x00isommp42")
    };

    let mut mvhd = Vec::new();
    for x in [qt_time, qt_time, timescale, duration, 0x0001_0000] {
        mvhd.extend_from_slice(&x.to_be_bytes());
    }
    mvhd.extend_from_slice(&0x0100u16.to_be_bytes());
    mvhd.extend_from_slice(&[0u8; 10]);
    for m in MATRIX {
        mvhd.extend_from_slice(&m.to_be_bytes());
    }
    mvhd.extend_from_slice(&[0u8; 24]);
    mvhd.extend_from_slice(&2u32.to_be_bytes());
    let mvhd = full(b"mvhd", 0, 0, &mvhd);

    let mut tkhd = Vec::new();
    for x in [qt_time, qt_time, 1, 0, duration] {
        tkhd.extend_from_slice(&x.to_be_bytes());
    }
    tkhd.extend_from_slice(&[0u8; 8]);
    tkhd.extend_from_slice(&[0u8; 4]); // layer, alternate group
    tkhd.extend_from_slice(&[0u8; 4]); // volume, reserved
    for m in MATRIX {
        tkhd.extend_from_slice(&m.to_be_bytes());
    }
    tkhd.extend_from_slice(&(v.width << 16).to_be_bytes());
    tkhd.extend_from_slice(&(v.height << 16).to_be_bytes());
    let tkhd = full(b"tkhd", 0, 3, &tkhd);

    let mut mdhd = Vec::new();
    for x in [qt_time, qt_time, timescale, duration] {
        mdhd.extend_from_slice(&x.to_be_bytes());
    }
    mdhd.extend_from_slice(&0x15C7u16.to_be_bytes());
    mdhd.extend_from_slice(&0u16.to_be_bytes());
    let mdhd = full(b"mdhd", 0, 0, &mdhd);

    let mut hdlr = Vec::new();
    hdlr.extend_from_slice(b"mhlrvideappl");
    hdlr.extend_from_slice(&[0u8; 8]);
    hdlr.extend_from_slice(b"\x0bVideoHandler");
    let hdlr = full(b"hdlr", 0, 0, &hdlr);

    let mut avc1 = Vec::new();
    avc1.extend_from_slice(&[0u8; 6]);
    avc1.extend_from_slice(&1u16.to_be_bytes());
    avc1.extend_from_slice(&[0u8; 16]);
    avc1.extend_from_slice(&u16::try_from(v.width).unwrap_or(u16::MAX).to_be_bytes());
    avc1.extend_from_slice(&u16::try_from(v.height).unwrap_or(u16::MAX).to_be_bytes());
    avc1.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    avc1.extend_from_slice(&0x0048_0000u32.to_be_bytes());
    avc1.extend_from_slice(&0u32.to_be_bytes());
    avc1.extend_from_slice(&1u16.to_be_bytes());
    let mut name = [0u8; 32];
    name[0] = 4;
    name[1..5].copy_from_slice(b"H264");
    avc1.extend_from_slice(&name);
    avc1.extend_from_slice(&0x0018u16.to_be_bytes());
    avc1.extend_from_slice(&0xFFFFu16.to_be_bytes());
    let avc1 = bx(b"avc1", &avc1);
    let mut stsd = 1u32.to_be_bytes().to_vec();
    stsd.extend_from_slice(&avc1);
    let stsd = full(b"stsd", 0, 0, &stsd);
    let frames = u32::try_from(v.duration_ms * 30 / 1000).unwrap_or(1).max(1);
    let mut stts = 1u32.to_be_bytes().to_vec();
    stts.extend_from_slice(&frames.to_be_bytes());
    stts.extend_from_slice(&20u32.to_be_bytes());
    let stts = full(b"stts", 0, 0, &stts);
    let stbl = bx(
        b"stbl",
        &[
            stsd,
            stts,
            full(b"stsc", 0, 0, &0u32.to_be_bytes()),
            full(b"stsz", 0, 0, &[0u8; 8]),
            full(b"stco", 0, 0, &0u32.to_be_bytes()),
        ]
        .concat(),
    );
    let vmhd = full(b"vmhd", 0, 1, &[0u8; 8]);
    let dref = full(
        b"dref",
        0,
        0,
        &[&1u32.to_be_bytes()[..], &full(b"url ", 0, 1, &[])].concat(),
    );
    let minf = bx(b"minf", &[vmhd, bx(b"dinf", &dref), stbl].concat());
    let mdia = bx(b"mdia", &[mdhd, hdlr, minf].concat());
    let trak = bx(b"trak", &[tkhd, mdia].concat());

    let mut udta = Vec::new();
    if let Some(m) = &v.make {
        udta.extend_from_slice(&qt_string(b"\xA9mak", m));
    }
    if let Some(m) = &v.model {
        udta.extend_from_slice(&qt_string(b"\xA9mod", m));
    }
    if let Some(l) = &v.location {
        udta.extend_from_slice(&qt_string(b"\xA9xyz", l));
    }
    let created = crate::camera::exif_datetime(v.created_utc);
    let iso = format!(
        "{}-{}-{}T{}Z",
        &created[0..4],
        &created[5..7],
        &created[8..10],
        &created[11..]
    );
    udta.extend_from_slice(&qt_string(b"\xA9day", &iso));

    // Apple-style metadata (moov/meta with mdta keys), as iPhones and many cameras write.
    let mut items: Vec<(&str, String)> = Vec::new();
    if let Some(m) = &v.make {
        items.push(("com.apple.quicktime.make", m.clone()));
    }
    if let Some(m) = &v.model {
        items.push(("com.apple.quicktime.model", m.clone()));
    }
    if let Some(l) = &v.location {
        items.push(("com.apple.quicktime.location.ISO6709", l.clone()));
    }
    if let Some(id) = &v.content_identifier {
        items.push(("com.apple.quicktime.content.identifier", id.clone()));
    }
    items.push((
        "com.apple.quicktime.creationdate",
        format!("{}+00:00", &iso[..iso.len() - 1]),
    ));
    let meta = meta_box(&items);
    let moov = bx(b"moov", &[mvhd, trak, bx(b"udta", &udta), meta].concat());

    let mut mdat = Vec::with_capacity(v.payload);
    let mut s = v.seed;
    while mdat.len() + 8 <= v.payload {
        s = mix64(s);
        mdat.extend_from_slice(&s.to_le_bytes());
    }
    mdat.resize(v.payload, 0);
    [ftyp, moov, bx(b"mdat", &mdat)].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_are_well_formed() {
        let v = write_video(&VideoSpec {
            quicktime: true,
            width: 1920,
            height: 1080,
            duration_ms: 12_000,
            created_utc: 1_600_000_000,
            make: Some("Apple".into()),
            model: Some("iPhone 12".into()),
            location: None,
            content_identifier: None,
            payload: 1000,
            seed: 1,
        });
        // Top-level boxes tile the file exactly.
        let mut i = 0;
        let mut kinds = Vec::new();
        while i < v.len() {
            let len = u32::from_be_bytes([v[i], v[i + 1], v[i + 2], v[i + 3]]) as usize;
            kinds.push(v[i + 4..i + 8].to_vec());
            i += len;
        }
        assert_eq!(i, v.len());
        assert_eq!(
            kinds,
            vec![b"ftyp".to_vec(), b"moov".to_vec(), b"mdat".to_vec()]
        );
    }
}

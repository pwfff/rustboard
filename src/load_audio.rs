use ebur128::EbuR128;
use rodio::{source::UniformSourceIterator, Decoder, Source};
use slugify::slugify;
use std::{
    fs::{DirEntry, File},
    io::BufReader,
};

use crate::pw_helper::{PlayBuf, DEFAULT_CHANNELS, DEFAULT_RATE, TARGET_LUFS};

fn db_to_amp(db: f32) -> f32 {
    10f32.powf(db / 20.)
}

fn amp_to_db(amp: f32) -> f32 {
    20. * amp.log10()
}

fn lufs_multiplier(current: f32, target: f32) -> f32 {
    db_to_amp(target) / db_to_amp(current)
}

pub async fn process_sound(path: DirEntry) -> (String, PlayBuf) {
    let key = slugify!(path.file_name().to_str().unwrap());

    //println!("loading {:?}", key);

    // Load a sound from a file, using a path relative to Cargo.toml
    let file = BufReader::new(File::open(path.path()).unwrap());

    // Decode that sound file into a source
    let source = Decoder::new(file).unwrap();

    // convert to known channels and sample rate
    let conv = UniformSourceIterator::<_, f32>::new(source, DEFAULT_CHANNELS as u16, DEFAULT_RATE)
        .buffered();

    // measure overall loudness
    let mut ebur = EbuR128::new(DEFAULT_CHANNELS, DEFAULT_RATE, ebur128::Mode::I).unwrap();
    let buf: Vec<f32> = conv.clone().collect();
    ebur.add_frames_f32(&buf).unwrap();
    //println!(
    //    "Integrated loudness: {:.1} LUFS",
    //    ebur.loudness_global().unwrap()
    //);

    // get float amp value from lufs
    let amp = lufs_multiplier(ebur.loudness_global().unwrap() as f32, TARGET_LUFS);
    let adjusted: Vec<f32> = buf.iter().map(|s| *s * amp).collect();

    // measure again
    //let mut ebur =
    //    EbuR128::new(DEFAULT_CHANNELS, DEFAULT_RATE, ebur128::Mode::I).unwrap();
    //ebur.add_frames_f32(&adjusted).unwrap();
    //println!(
    //    "Integrated loudness: {:.1} LUFS",
    //    ebur.loudness_global().unwrap()
    //);

    (key.clone(), PlayBuf { buf: adjusted })
}

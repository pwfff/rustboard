use crate::pw_helper::{DEFAULT_CHANNELS, DEFAULT_RATE};
use pipewire as pw;
use rodio::source::UniformSourceIterator;
use rodio::{Decoder, Source};
use std::fs::File;
use std::io::BufReader;
use std::thread;

mod pw_helper;
use pw_helper::{PlayBuf, pw_thread};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    pw::init();


    // Load a sound from a file, using a path relative to Cargo.toml
    let file = BufReader::new(File::open("/home/pwf/BEEG BEEG YOSHI.mp3").unwrap());
    // Decode that sound file into a source
    let source = Decoder::new(file).unwrap().buffered();
    let conv = UniformSourceIterator::<_, i16>::new(source, DEFAULT_CHANNELS as u16, DEFAULT_RATE)
        .buffered();
    let buf: Vec<i16> = conv.collect();

    println!("read file");

    let (pw_sender, pw_receiver) = pipewire::channel::channel();

    let pw_thread = thread::spawn(move || pw_thread(pw_receiver));

    pw_sender.send(PlayBuf{
        target: "".to_owned(),
        buf,
        done: false,
    }).expect("bad send i guess");

    pw_thread.join().expect("idk");

    Ok(())
}

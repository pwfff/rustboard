use crate::pw_helper::{DEFAULT_CHANNELS, DEFAULT_RATE};
use axum::body::Bytes;
use axum::extract::State;
use axum::{http::StatusCode, routing::get, Router};
use pipewire as pw;
use rodio::source::UniformSourceIterator;
use rodio::{Decoder, Source};
use std::fs::File;
use std::io::BufReader;
use std::sync::{Arc, RwLock};

mod pw_helper;
use pw_helper::{pw_thread, PlayBuf};

struct MyState {
    playbuf: PlayBuf,
    pw_sender: pipewire::channel::Sender<PlayBuf>,
}

type SharedState = Arc<RwLock<MyState>>;

async fn play(State(state): State<SharedState>) -> Result<Bytes, StatusCode> {
    let state = state.read().unwrap();
    let sender = &state.pw_sender;
    let playbuf = &state.playbuf;
    sender.send(playbuf.clone()).expect("oops");
    Ok("playing".into())
}

#[tokio::main]
async fn main() {
    std::env::set_var("RUST_LOG", "debug");

    // initialize tracing
    tracing_subscriber::fmt::init();

    pw::init();

    let (pw_sender, pw_receiver) = pipewire::channel::channel();
    let pw_thread = tokio::spawn(async move { pw_thread(pw_receiver) });

    // Load a sound from a file, using a path relative to Cargo.toml
    let file = BufReader::new(
        File::open("/home/pwf/Documents/Why you heff to be mad？ (Original) [xzpndHtdl9A].wav")
            .unwrap(),
    );
    // Decode that sound file into a source
    let source = Decoder::new(file).unwrap().buffered();
    let conv = UniformSourceIterator::<_, i16>::new(source, DEFAULT_CHANNELS as u16, DEFAULT_RATE)
        .buffered();
    let buf: Vec<i16> = conv.collect();

    let shared_state = SharedState::new(RwLock::new(MyState {
        pw_sender: pw_sender.clone(),
        playbuf: PlayBuf { buf: buf.to_vec() },
    }));

    let app = Router::new().route("/", get(play).with_state(shared_state));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

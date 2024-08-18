use crate::pw_helper::{DEFAULT_CHANNELS, DEFAULT_RATE};
use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use pipewire as pw;
use rodio::source::UniformSourceIterator;
use rodio::{Decoder, Source};
use std::fs::File;
use std::io::BufReader;

mod pw_helper;
use pw_helper::{pw_thread, PlayBuf};

struct MyState {
    playbuf: PlayBuf,
    pw_sender: pipewire::channel::Sender<PlayBuf>,
}

#[get("/")]
async fn hello(data: web::Data<MyState>) -> impl Responder {
    data.pw_sender.send(data.playbuf.clone()).expect("oops");
    HttpResponse::Ok().body("Hello world!")
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    std::env::set_var("RUST_LOG", "debug");
    env_logger::init();

    pw::init();

    let (pw_sender, pw_receiver) = pipewire::channel::channel();
    let pw_thread = actix_web::rt::spawn(async move { pw_thread(pw_receiver) });

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

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(MyState {
                pw_sender: pw_sender.clone(),
                playbuf: PlayBuf { buf: buf.to_vec() },
            }))
            .service(hello)
    })
    .shutdown_timeout(3)
    .bind(("127.0.0.1", 8081))?
    .run()
    .await
    .expect("idk");

    Ok(())
}

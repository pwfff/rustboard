use crate::pw_helper::{DEFAULT_CHANNELS, DEFAULT_RATE};
use askama::Template;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::{Html, IntoResponse, Response};
use axum::{http::StatusCode, routing::get, routing::post, Router};
use pipewire as pw;
use rodio::source::UniformSourceIterator;
use rodio::{Decoder, Source};
use serde::{Deserialize, Serialize};
use slugify::slugify;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::sync::{Arc, RwLock};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod pw_helper;
use pw_helper::{pw_thread, PlayBuf};

struct MyState {
    playbufs: HashMap<String, PlayBuf>,
    pw_sender: pipewire::channel::Sender<PlayBuf>,
}

impl MyState {
    fn new(pw_sender: pipewire::channel::Sender<PlayBuf>) -> Self {
        let mut playbufs = HashMap::new();

        let dir = std::fs::read_dir("./sounds").unwrap();
        for path in dir {
            if let Ok(path) = path {
                if path.file_name() == "__placeholder__" {
                    continue;
                }

                let key = slugify!(path.file_name().to_str().unwrap());

                // Load a sound from a file, using a path relative to Cargo.toml
                let file = BufReader::new(File::open(path.path()).unwrap());
                // Decode that sound file into a source
                let source = Decoder::new(file).unwrap().buffered();
                let conv = UniformSourceIterator::<_, i16>::new(
                    source,
                    DEFAULT_CHANNELS as u16,
                    DEFAULT_RATE,
                )
                .buffered();
                let buf: Vec<i16> = conv.collect();
                playbufs.insert(key, PlayBuf { buf });
            }
        }

        MyState {
            playbufs,
            pw_sender,
        }
    }
}

type SharedState = Arc<RwLock<MyState>>;

async fn play(
    Path(key): Path<String>,
    State(state): State<SharedState>,
) -> Result<Bytes, StatusCode> {
    let state = state.read().unwrap();
    let sender = &state.pw_sender;
    let playbufs = &state.playbufs;
    if let Some(playbuf) = playbufs.get(&key) {
        sender.send(playbuf.clone()).expect("oops");
        return Ok("playing".into());
    }

    Err(StatusCode::NOT_FOUND)
}

#[derive(Debug, Serialize, Deserialize)]
struct Playable {
    key: String,
}

#[derive(Template)]
#[template(path = "index.html")]
struct SoundsTemplate {
    sounds: Vec<String>,
}

struct HtmlTemplate<T>(T);

impl<T> IntoResponse for HtmlTemplate<T>
where
    T: Template,
{
    fn into_response(self) -> Response {
        match self.0.render() {
            Ok(html) => {
                Html(html).into_response()
            }
            Err(err) => {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to render template. Error: {err}"),
                )
                    .into_response()
            }
        }
    }
}

async fn list(State(state): State<SharedState>) -> impl IntoResponse {
    let state = state.read().unwrap();
    let playbufs = &state.playbufs;

    let template = SoundsTemplate {
        sounds: playbufs.keys().map(|k| k.clone()).collect(),
    };

    HtmlTemplate(template)
}

#[tokio::main]
async fn main() {
    std::env::set_var("RUST_LOG", "debug");

    // initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "example_templates=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    pw::init();

    let (pw_sender, pw_receiver) = pipewire::channel::channel();
    let _pw_thread = tokio::spawn(async move { pw_thread(pw_receiver) });

    let shared_state = SharedState::new(RwLock::new(MyState::new(pw_sender.clone())));
    let app = Router::new()
        .route(
            "/sounds/:key",
            post(play).with_state(Arc::clone(&shared_state)),
        )
        .route("/", get(list).with_state(Arc::clone(&shared_state)));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();

    // TODO: quit signal and cleanup for pipewire stuff
}

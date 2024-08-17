#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use crate::pw_helper::{DEFAULT_CHANNELS, DEFAULT_RATE};
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use ashpd::WindowIdentifier;
use futures::stream::StreamExt;
use pipewire as pw;
use rodio::source::UniformSourceIterator;
use rodio::{Decoder, Source};
use std::fs::File;
use std::io::BufReader;
use tauri::{Emitter, Listener, Manager};

mod pw_helper;
use pw_helper::{pw_thread, PlayBuf};

struct MyState {
    playbuf: PlayBuf,
    pw_sender: pipewire::channel::Sender<PlayBuf>,
    globals: GlobalShortcuts<'static>,
}

#[tauri::command]
async fn greet(state: tauri::State<'_, MyState>) -> Result<(), String> {
    state
        .pw_sender
        .send(state.playbuf.clone())
        .expect("bad send i guess");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    pw::init();

    let (pw_sender, pw_receiver) = pipewire::channel::channel();

    let pw_thread = tokio::spawn(async move { pw_thread(pw_receiver) });

    let globals = GlobalShortcuts::new()
        .await
        .expect("couldnt make new globalshortcuts");
    let session = globals
        .create_session()
        .await
        .expect("couldnt create session");
    //let window = app.get_window("rustboard").expect("no main window");
    //let handle = window.window_handle().expect("no handle i guess").as_raw();
    let window = WindowIdentifier::None;
    //let window = WindowIdentifier::from_raw_handle(&handle, None).await;
    let shortcut = NewShortcut::new("rustboard-hotkey", "hotkey 1 for rustboard")
        .preferred_trigger("ALT+SHIFT+a");
    globals
        .bind_shortcuts(&session, &[shortcut], &window)
        .await
        .expect("couldnt bind shortcuts");

    let mut globals_stream = globals
        .receive_activated()
        .await
        .expect("couldnt start receivin");

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

    let app = tauri::Builder::default()
        .manage(MyState {
            pw_sender,
            globals,
            playbuf: PlayBuf { buf },
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    if let Some(_) = globals_stream.next().await {
                        match handle.emit("hotkey-triggered", "activated") {
                            Ok(_) => {}
                            Err(e) => {
                                println!("error emitting event {:?}", e)
                            }
                        };
                    }
                }
            });

            let handle = app.handle().clone();
            app.listen("hotkey-triggered", move |_| {
                let state: tauri::State<MyState> = handle.state();
                state
                    .pw_sender
                    .send(state.playbuf.clone())
                    .expect("bad send i guess");
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!());

    pw_thread.await.expect("cool");

    app.expect("poop");

    Ok(())
}

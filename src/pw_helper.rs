use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::mpsc,
    time::Duration,
};

use pipewire::{
    context::Context,
    core::{Core, PW_ID_CORE},
    main_loop::MainLoop,
    properties::properties,
    spa::{
        param::audio::{AudioFormat, AudioInfoRaw, MAX_CHANNELS},
        pod::{serialize::PodSerializer, Object, Pod, Value},
        utils::Direction,
    },
    stream::{Stream, StreamFlags, StreamListener},
    types::ObjectType,
};

pub const DEFAULT_RATE: u32 = 44100;
pub const DEFAULT_CHANNELS: u32 = 2;
pub const DEFAULT_VOLUME: f64 = 0.7;
pub const CHAN_SIZE: usize = std::mem::size_of::<i16>();

#[derive(Debug)]
struct Done;

#[derive(Debug)]
pub struct PlayBuf {
    pub target: String,
    pub buf: Vec<i16>,
}

struct State {
    factory: Option<String>,
    loopback_node: Option<String>,
    discord_node: Option<String>,
    players: Vec<Player>,
}

struct Player {
    stream: Stream,
    listener: StreamListener<usize>,
    done: mpsc::Receiver<Done>,
}

impl State {
    fn new() -> Self {
        State {
            factory: None,
            loopback_node: None,
            discord_node: None,
            players: vec![],
        }
    }

    fn set_factory(&mut self, factory: String) {
        self.factory = Some(factory);
    }

    fn set_loopback(&mut self, loopback_node: Option<String>) {
        self.loopback_node = loopback_node;
    }

    fn set_discord(&mut self, discord_node: Option<String>) {
        self.discord_node = discord_node;
    }

    fn add_player(&mut self, player: Player) {
        self.players.push(player);
    }
}

pub fn pw_thread(pw_receiver: pipewire::channel::Receiver<PlayBuf>) {
    let mainloop = MainLoop::new(None).expect("failed to create main loop");
    let context = Context::new(&mainloop).expect("failed to create context");
    let core = context.connect(None).expect("failed to connect to core");
    let registry = core.get_registry().expect("failed to get registry");

    let state: Rc<RefCell<State>> = Rc::new(RefCell::new(State::new()));

    // first set up listener. this will maintain our state so we always have the latest node IDs
    let state_clone = state.clone();
    let _listener = registry
        .add_listener_local()
        .global(move |global| {
            if let Some(props) = global.props {
                if props.get("factory.type.name") == Some(ObjectType::Link.to_str()) {
                    let factory_name = props.get("factory.name").expect("Factory has no name");
                    state_clone.borrow_mut().set_factory(factory_name.into());
                    return;
                }

                if let Some(alias) = props.get("port.alias") {
                    if alias.contains("WEBRTC") {
                        if props.get("port.direction") != Some("in") {
                            return;
                        }
                        println!("got discord");
                        state_clone.borrow_mut().set_discord(Some(
                            props.get("node.id").expect("node id nope").to_owned(),
                        ));
                    }

                    if alias.contains("MOMENTUM") {
                        if props.get("port.direction") != Some("out") {
                            return;
                        }
                        println!("got loopback");
                        state_clone.borrow_mut().set_loopback(Some(
                            props.get("node.id").expect("node id nope").to_owned(),
                        ));
                    }
                }
            }
        })
        .register();

    // Process all pending events to get the factory.
    do_roundtrip(&mainloop, &core);

    // setup listener for play events
    let state_clone = state.clone();
    let _receiver = pw_receiver.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |playbuf| {
            let (done_tx, done_rx) = mpsc::channel();
            println!("got event");
            let mut state = state_clone.borrow_mut();
            let mut maybe_node_id = &state.discord_node;
            if maybe_node_id.is_none() {
                println!("node was none, using loopback");
                maybe_node_id = &state.loopback_node;
                if maybe_node_id.is_none() {
                    println!("node was none");
                    return;
                }
            }
            let node_id = maybe_node_id.as_ref().expect("umn?");

            let stream = Stream::new(
                &core,
                "rustboard-src",
                properties! {
                    *pipewire::keys::MEDIA_TYPE => "Audio",
                    *pipewire::keys::MEDIA_ROLE => "Music",
                    *pipewire::keys::MEDIA_CATEGORY => "Playback",
                    *pipewire::keys::AUDIO_CHANNELS => "2",
                },
            )
            .expect("couldnt create stream");

            let cursor: usize = 0;
            let done_tx_clone = done_tx.clone();
            let listener = stream
                .add_local_listener_with_user_data(cursor)
                .process(move |stream, cursor| {
                    match stream.dequeue_buffer() {
                        None => println!("No buffer received"),
                        Some(mut buffer) => {
                            let datas = buffer.datas_mut();
                            let buf = &playbuf.buf;
                            if *cursor > buf.len() {
                                for data in datas {
                                    if let Some(slice) = data.data() {
                                        slice.fill_with(|| 0);
                                    }
                                }
                                done_tx_clone.send(Done).expect("couldnt notify done");
                                println!("dead stream bro");
                                return;
                                //*cursor = 0;
                            }
                            let stride = DEFAULT_CHANNELS as usize;
                            let mut n_frames = 0;
                            for data in datas {
                                n_frames = if let Some(slice) = data.data() {
                                    let n_frames = slice.len() / CHAN_SIZE;
                                    let start = *cursor;
                                    let end = (n_frames + *cursor).min(buf.len());
                                    println!("n_frames {n_frames:#?} cursor {cursor:#?}");
                                    let sample: Vec<u8> = buf[start..end]
                                        .into_iter()
                                        .map(|v| i16::to_le_bytes(*v))
                                        .flatten()
                                        .collect();
                                    slice[..sample.len()].copy_from_slice(&sample);
                                    n_frames
                                } else {
                                    0
                                };

                                let chunk = data.chunk_mut();
                                *chunk.offset_mut() = 0;
                                *chunk.stride_mut() = stride as _;
                                *chunk.size_mut() = (stride * n_frames) as _;
                            }

                            *cursor += n_frames;
                        }
                    }
                })
                .register()
                .expect("couldnt register stream listener");

            let mut audio_info = AudioInfoRaw::new();
            audio_info.set_format(AudioFormat::S16LE);
            audio_info.set_rate(DEFAULT_RATE);
            audio_info.set_channels(DEFAULT_CHANNELS);
            let mut position = [0; MAX_CHANNELS];
            position[0] = libspa_sys::SPA_AUDIO_CHANNEL_FL;
            position[1] = libspa_sys::SPA_AUDIO_CHANNEL_FR;
            audio_info.set_position(position);

            let values: Vec<u8> = PodSerializer::serialize(
                std::io::Cursor::new(Vec::new()),
                &Value::Object(Object {
                    type_: libspa_sys::SPA_TYPE_OBJECT_Format,
                    id: libspa_sys::SPA_PARAM_EnumFormat,
                    properties: audio_info.into(),
                }),
            )
            .unwrap()
            .0
            .into_inner();

            let mut params = [Pod::from_bytes(&values).unwrap()];

            let node_id: u32 = node_id.parse().expect("wasnt u32");

            println!("connecting stream? {:#?}", node_id);
            stream
                .connect(
                    Direction::Output,
                    Some(node_id),
                    StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
                    &mut params,
                )
                .expect("did no connect");
            println!("connected stream? {:#?}", node_id);

            let player = Player {
                stream,
                listener,
                done: done_rx,
            };
            state.add_player(player);

            //let stream_cell = Rc::new(RefCell::new(None));
            //let listener_cell = Rc::new(RefCell::new(None));
            //stream_cell.replace(Some(stream));
            //listener_cell.replace(Some(listener));
            //let _attached = done_rx.attach(mainloop.loop_(), move |_| {
            //    println!("done_rx");
            //    stream_cell.take().expect("um").disconnect().expect("idk");
            //    listener_cell.take().expect("um").unregister();
            //});
        }
    });

    let state_clone = state.clone();
    let timer = mainloop.loop_().add_timer(move |_| {
        let mut state = state_clone.borrow_mut();

        state.players.retain(|player| {
            if let Ok(_) = player.done.try_recv() {
                println!("disconnecting");
                //let player = player.to_owned();
                //player.stream.disconnect().expect("ok");
                //player.listener.unregister();
                false
            } else {
                true
            }
        });
    });
    timer.update_timer(
        Some(Duration::from_millis(10)),
        Some(Duration::from_millis(100)),
    );

    mainloop.run();
}

/// Do a single roundtrip to process all events.
/// See the example in roundtrip.rs for more details on this.
fn do_roundtrip(mainloop: &MainLoop, core: &Core) {
    let done = Rc::new(Cell::new(false));
    let done_clone = done.clone();
    let loop_clone = mainloop.clone();

    // Trigger the sync event. The server's answer won't be processed until we start the main loop,
    // so we can safely do this before setting up a callback. This lets us avoid using a Cell.
    let pending = core.sync(0).expect("sync failed");

    let _listener_core = core
        .add_listener_local()
        .done(move |id, seq| {
            if id == PW_ID_CORE && seq == pending {
                done_clone.set(true);
                loop_clone.quit();
            }
        })
        .register();

    while !done.get() {
        mainloop.run();
    }
}

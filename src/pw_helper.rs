use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::mpsc,
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
pub const TARGET_LUFS: f32 = -16.0;
pub const CHAN_SIZE: usize = std::mem::size_of::<f32>();

#[derive(Debug)]
struct Done;

#[derive(Debug)]
pub enum Message {
    Play(PlayBuf),
    Stop(),
}

#[derive(Debug, Clone)]
pub struct PlayBuf {
    pub buf: Vec<u8>,
}

impl PlayBuf {
    pub fn new(f32s: Vec<f32>) -> Self {
        Self {
            buf: f32s.iter().flat_map(|v| v.to_le_bytes()).collect(),
        }
    }
}

struct Player {
    _stream: Stream,
    _listener: StreamListener<usize>,
    done_tx: mpsc::Sender<Done>,
    done_rx: mpsc::Receiver<Done>,
}

struct State {
    factory: Option<String>,
    loopback_node: Option<u32>,
    discord_node: Option<u32>,
    deadlock_node: Option<u32>,
    players: Vec<Player>,
}

impl State {
    fn new() -> Self {
        State {
            factory: None,
            loopback_node: None,
            discord_node: None,
            deadlock_node: None,
            players: vec![],
        }
    }

    fn set_factory(&mut self, factory: String) {
        self.factory = Some(factory);
    }

    fn set_loopback(&mut self, loopback_node: Option<u32>) {
        self.loopback_node = loopback_node;
    }

    fn set_deadlock(&mut self, deadlock_node: Option<u32>) {
        self.deadlock_node = deadlock_node;
    }

    fn set_discord(&mut self, discord_node: Option<u32>) {
        self.discord_node = discord_node;
    }

    fn add_player(&mut self, player: Player) {
        self.players.push(player);
    }
}

pub fn pw_thread(pw_receiver: pipewire::channel::Receiver<Message>) {
    let mainloop = MainLoop::new(None).expect("failed to create main loop");
    let context = Context::new(&mainloop).expect("failed to create context");
    let core = context.connect(None).expect("failed to connect to core");
    let registry = core.get_registry().expect("failed to get registry");

    let state: Rc<RefCell<State>> = Rc::new(RefCell::new(State::new()));

    // first set up listener. this will maintain our state so we always have the latest node IDs
    let _listener = registry
        .add_listener_local()
        .global({
            let state = state.clone();
            move |global| {
                if let Some(props) = global.props {
                    if props.get("factory.type.name") == Some(ObjectType::Link.to_str()) {
                        let factory_name = props.get("factory.name").expect("Factory has no name");
                        state.borrow_mut().set_factory(factory_name.into());
                        return;
                    }

                    if let Some(alias) = props.get("port.alias") {
                        if alias.contains("project8") {
                            if props.get("port.direction") != Some("in") {
                                return;
                            }
                            tracing::debug!("got project8");
                            state.borrow_mut().set_deadlock(Some(
                                props
                                    .get("node.id")
                                    .expect("node id nope")
                                    .parse()
                                    .expect("couldnt parse deadlock node"),
                            ));
                        }

                        if alias.contains("WEBRTC") {
                            if props.get("port.direction") != Some("in") {
                                return;
                            }
                            tracing::debug!("got discord");
                            state.borrow_mut().set_discord(Some(
                                props
                                    .get("node.id")
                                    .expect("node id nope")
                                    .parse()
                                    .expect("couldnt parse discord node"),
                            ));
                        }

                        if alias.contains("MOMENTUM") {
                            if props.get("port.direction") != Some("out") {
                                return;
                            }
                            tracing::debug!("got loopback");
                            state.borrow_mut().set_loopback(Some(
                                props
                                    .get("node.id")
                                    .expect("node id nope")
                                    .parse()
                                    .expect("couldnt parse loopback node"),
                            ));
                        }
                    }
                }
            }
        })
        .register();

    // Process all pending events to get the factory.
    do_roundtrip(&mainloop, &core);

    let _idle = mainloop.loop_().add_idle(true, {
        let state = state.clone();
        move || {
            state.borrow_mut().players.retain(|player| {
                if player.done_rx.try_recv().is_ok() {
                    tracing::debug!("disconnecting");
                    false
                } else {
                    true
                }
            });
        }
    });

    // setup listener for play events
    let _receiver = pw_receiver.attach(mainloop.loop_(), {
        {
            let state = state.clone();
            move |message| match message {
                Message::Play(playbuf) => {
                    tracing::debug!("got event play");
                    let nodes: Vec<u32> = [
                        state.borrow().discord_node,
                        state.borrow().loopback_node,
                        state.borrow().deadlock_node,
                    ]
                    .iter()
                    .filter_map(|&n| n)
                    .collect();

                    for node in nodes {
                        play_to_node(&core, state.clone(), playbuf.clone(), node);
                    }
                }
                Message::Stop() => {
                    for p in state.borrow().players.iter() {
                        p.done_tx.send(Done).unwrap();
                    }
                }
            }
        }
    });

    mainloop.run();
}

fn play_to_node(core: &Core, state: Rc<RefCell<State>>, playbuf: PlayBuf, node_id: u32) {
    // audio info will be the same across all players, so might as well set this crap up now
    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::F32LE);
    audio_info.set_rate(DEFAULT_RATE);
    audio_info.set_channels(DEFAULT_CHANNELS);
    let mut position = [0; MAX_CHANNELS];
    position[0] = libspa_sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = libspa_sys::SPA_AUDIO_CHANNEL_FR;
    audio_info.set_position(position);

    let stream = Stream::new(
        core,
        "rustboard-src",
        properties! {
            *pipewire::keys::MEDIA_TYPE => "Audio",
            *pipewire::keys::MEDIA_ROLE => "Music",
            *pipewire::keys::MEDIA_CATEGORY => "Playback",
            *pipewire::keys::AUDIO_CHANNELS => "2",
        },
    )
    .expect("couldnt create stream");

    // cursor to track how far through the buffer we are
    let cursor: usize = 0;
    // channel for telling timer when to drop the stream/listener
    let (done_tx, done_rx) = mpsc::channel();
    let done_tx_clone = done_tx.clone();
    let _listener = stream
        .add_local_listener_with_user_data(cursor)
        .process(move |stream, cursor| {
            let buf = &playbuf.buf;
            match stream.dequeue_buffer() {
                None => tracing::error!("No buffer received"),
                Some(mut buffer) => {
                    let datas = buffer.datas_mut();
                    let stride = DEFAULT_CHANNELS as usize;
                    let mut n_frames = 0;
                    for data in datas {
                        if *cursor >= buf.len() {
                            //println!("dead stream bro");
                            if let Some(slice) = data.data() {
                                slice.fill(0);
                            }
                            done_tx_clone.send(Done).expect("couldnt notify done");
                        } else {
                            n_frames = if let Some(slice) = data.data() {
                                let n_frames = slice.len() / CHAN_SIZE;
                                let start = *cursor;
                                let end = (*cursor + slice.len()).min(buf.len());
                                let size = end - start;
                                slice[..size].copy_from_slice(&buf[start..end]);
                                if size < slice.len() {
                                    slice[size..].fill(0);
                                }
                                n_frames
                            } else {
                                0
                            };
                        }

                        let chunk = data.chunk_mut();
                        *chunk.offset_mut() = 0;
                        *chunk.stride_mut() = stride as _;
                        *chunk.size_mut() = (n_frames * CHAN_SIZE) as _;
                    }

                    *cursor += n_frames * CHAN_SIZE;
                }
            }
        })
        .register()
        .expect("couldnt register stream listener");

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

    stream
        .connect(
            Direction::Output,
            Some(node_id),
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
            &mut params,
        )
        .expect("did no connect");

    state.borrow_mut().add_player(Player {
        _listener,
        _stream: stream,
        done_rx,
        done_tx,
    });
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

use crate::spa::pod::Pod;
use pipewire as pw;
use pw::{context::Context, main_loop::MainLoop, properties::properties, spa, types::ObjectType};
use std::cell::Cell;
use std::{cell::OnceCell, rc::Rc};

pub const DEFAULT_RATE: u32 = 44100;
pub const DEFAULT_CHANNELS: u32 = 2;
pub const DEFAULT_VOLUME: f64 = 0.7;
pub const PI_2: f64 = std::f64::consts::PI + std::f64::consts::PI;
pub const CHAN_SIZE: usize = std::mem::size_of::<i16>();

fn main() -> Result<(), Box<dyn std::error::Error>> {
    pw::init();

    let mainloop = MainLoop::new(None)?;
    let context = Context::new(&mainloop)?;
    let core = context.connect(None)?;
    let registry = core.get_registry()?;

    // Setup a registry listener that will obtain the name of a link factory and write it into `factory`.
    let factory: Rc<OnceCell<String>> = Rc::new(OnceCell::new());
    let factory_clone = factory.clone();

    // TODO: once cell, but disconnect/reconnect changes node ids?
    let discord_node_id: Rc<OnceCell<String>> = Rc::new(OnceCell::new());
    let discord_fl: Rc<OnceCell<String>> = Rc::new(OnceCell::new());
    let discord_fr: Rc<OnceCell<String>> = Rc::new(OnceCell::new());
    let discord_node_id_clone = discord_node_id.clone();
    let discord_fl_clone = discord_fl.clone();
    let discord_fr_clone = discord_fr.clone();
    let mainloop_clone = mainloop.clone();

    let reg_listener = registry
        .add_listener_local()
        .global(move |global| {
            if let Some(props) = global.props {
                if props.get("factory.type.name") == Some(ObjectType::Link.to_str()) {
                    let factory_name = props.get("factory.name").expect("Factory has no name");
                    factory_clone
                        .set(factory_name.to_owned())
                        .expect("Factory name already set");
                    return;
                }

                if let Some(alias) = props.get("port.alias") {
                    if alias.contains("WEBRTC") {
                        if props.get("port.direction") != Some("in") {
                            return;
                        }
                        println!("{:#?}", props);
                        if discord_node_id_clone.get() == None {
                            discord_node_id_clone
                                .set(props.get("node.id").expect("node id nope").to_owned())
                                .expect("node id no set");
                        }
                        match props.get("audio.channel").expect("missing audio channel") {
                            "FL" => {
                                if discord_fl_clone.get() == None {
                                    discord_fl_clone
                                        .set(
                                            props
                                                .get("port.id")
                                                .expect("fl port id nope")
                                                .to_owned(),
                                        )
                                        .expect("double set fl");
                                }
                            }
                            "FR" => {
                                if discord_fr_clone.get() == None {
                                    discord_fr_clone
                                        .set(
                                            props
                                                .get("port.id")
                                                .expect("fr port id nope")
                                                .to_owned(),
                                        )
                                        .expect("double set fr");
                                }
                            }
                            _ => (),
                        };
                    }
                }
            }
        })
        .register();

    // Calling the `destroy_global` method on the registry will destroy the object with the specified id on the remote.
    // We don't have a specific object to destroy now, so this is commented out.
    // registry.destroy_global(313).into_result()?;

    // Process all pending events to get the factory.
    do_roundtrip(&mainloop, &core);

    // Now that we have our factory, we are no longer interested in any globals from the registry,
    // so we unregister the listener by dropping it.
    std::mem::drop(reg_listener);

    let data: f64 = 0.0;

    let stream = pw::stream::Stream::new(
        &core,
        "rustboard-src",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::AUDIO_CHANNELS => "2",
        },
    )?;

    let _listener = stream
        .add_local_listener_with_user_data(data)
        .process(|stream, acc| match stream.dequeue_buffer() {
            None => println!("No buffer received"),
            Some(mut buffer) => {
                let datas = buffer.datas_mut();
                let stride = CHAN_SIZE * DEFAULT_CHANNELS as usize;
                let data = &mut datas[0];
                let n_frames = if let Some(slice) = data.data() {
                    let n_frames = slice.len() / stride;
                    for i in 0..n_frames {
                        *acc += PI_2 * 440.0 / DEFAULT_RATE as f64;
                        if *acc >= PI_2 {
                            *acc -= PI_2
                        }
                        let val = (f64::sin(*acc) * DEFAULT_VOLUME * 16767.0) as i16;
                        for c in 0..DEFAULT_CHANNELS {
                            let start = i * stride + (c as usize * CHAN_SIZE);
                            let end = start + CHAN_SIZE;
                            let chan = &mut slice[start..end];
                            chan.copy_from_slice(&i16::to_le_bytes(val));
                        }
                    }
                    n_frames
                } else {
                    0
                };
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = stride as _;
                *chunk.size_mut() = (stride * n_frames) as _;
            }
        })
        .register()?;

    let node_id: u32 = discord_node_id
        .get()
        .expect("somehow no node id")
        .parse()
        .expect("wasnt u32");

    let mut audio_info = spa::param::audio::AudioInfoRaw::new();
    audio_info.set_format(spa::param::audio::AudioFormat::S16LE);
    audio_info.set_rate(DEFAULT_RATE);
    audio_info.set_channels(DEFAULT_CHANNELS);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = libspa_sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = libspa_sys::SPA_AUDIO_CHANNEL_FR;
    audio_info.set_position(position);

    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(pw::spa::pod::Object {
            type_: libspa_sys::SPA_TYPE_OBJECT_Format,
            id: libspa_sys::SPA_PARAM_EnumFormat,
            properties: audio_info.into(),
        }),
    )
    .unwrap()
    .0
    .into_inner();

    let mut params = [Pod::from_bytes(&values).unwrap()];

    stream.connect(
        spa::utils::Direction::Output,
        Some(node_id),
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    ).expect("did no connect");

    // Now that we have the name of a link factory, we can create an object with it!
    let link = core
        .create_object::<pw::link::Link>(
            factory.get().expect("No link factory found"),
            &pw::properties::properties! {
                "link.output.port" => "1",
                "link.input.port" => "2",
                "link.output.node" => "3",
                "link.input.node" => "4",
                // Don't remove the object on the remote when we destroy our proxy.
                "object.linger" => "1"
            },
        )
        .expect("Failed to create object");

    mainloop.run();

    Ok(())
}

/// Do a single roundtrip to process all events.
/// See the example in roundtrip.rs for more details on this.
fn do_roundtrip(mainloop: &pw::main_loop::MainLoop, core: &pw::core::Core) {
    let done = Rc::new(Cell::new(false));
    let done_clone = done.clone();
    let loop_clone = mainloop.clone();

    // Trigger the sync event. The server's answer won't be processed until we start the main loop,
    // so we can safely do this before setting up a callback. This lets us avoid using a Cell.
    let pending = core.sync(0).expect("sync failed");

    let _listener_core = core
        .add_listener_local()
        .done(move |id, seq| {
            if id == pw::core::PW_ID_CORE && seq == pending {
                done_clone.set(true);
                loop_clone.quit();
            }
        })
        .register();

    while !done.get() {
        mainloop.run();
    }
}

use clap::Parser;
use gilrs::{Button, Event, EventType, Gilrs};
use num::complex::Complex;
use std::f32::consts::PI;
use std::thread::sleep;
use std::time::Duration;
use tokio;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::watch;

use futuresdr::futuredsp::fir::NonResamplingFirKernel;
use futuresdr::anyhow::Result;
use futuresdr::async_io::block_on;
use futuresdr::async_net::UdpSocket;
use futuresdr::blocks::{Fir, FirBuilder};
use futuresdr::log::{debug, info, warn};
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::Runtime;

use util_blocks::AWGNComplex32;
use multitrx::Complex32Deserializer;
use multitrx::Complex32Serializer;
use multitrx::TcpSink;
use multitrx::TcpSource;

const PAD_FRONT: usize = 10000;
const PAD_TAIL: usize = 10000;

const STATION_X: f32 = 0.0;
const STATION_Y: f32 = 0.0;
const STATION_Z: f32 = 1.5;
const MAX_TAPS: usize = 41;

const TAP_VALUE_NO_LOSS: f32 = 32767.0;
// const MAGIC_SCALING_COEFF: f32 = 140.5;
const MAGIC_SCALING_COEFF: f32 = 155.0;
const TAP_VALUE_MAX: i16 = 10000;
const TAP_VALUE_MIN: i16 = -10000;

const SPEED_OF_LIGHT: f32 = 299_792_458.;
const LAMBDA: f32 = SPEED_OF_LIGHT / 2.45e9;

#[derive(Parser, Debug)]
#[clap(version)]
struct Args {
    /// AG TCPExchanger local sink port
    #[clap(long, value_parser)]
    local_tcp_sink_port_ag: u32,
    /// AG TCPExchanger remote sink socket address
    #[clap(long, value_parser)]
    remote_tcp_sink_address_ag: String,
    /// GA TCPExchanger local sink port
    #[clap(long, value_parser)]
    local_tcp_sink_port_ga: u32,
    /// GA TCPExchanger remote sink socket address
    #[clap(long, value_parser)]
    remote_tcp_sink_address_ga: String,
    /// UDP port to receive position updates
    #[clap(short, long, default_value_t = 1337)]
    local_udp_port: u32,
    /// UDP port to receive position updates
    #[clap(short, long, default_value_t = 1341)]
    model_selection_udp_port: u32,
    /// UDP port of channel emulator
    #[clap(short, long, default_value_t = 1338)]
    chanem_port: u32,
    /// Sample Rate
    #[clap(long, default_value_t = 200e6)]
    sample_rate: f64,
    /// Receive noise power
    #[clap(long, value_parser)]
    rx_noise_power: f32,
}

fn distance(x: f32, y: f32, z: f32) -> f32 {
    ((STATION_X - x).powi(2) + (STATION_Y - y).powi(2) + (STATION_Z - z).powi(2)).sqrt()
}

fn calculate_taps_freespace(
    x: f32,
    y: f32,
    z: f32,
    magic_scaling_coeff: f32,
) -> [i16; MAX_TAPS * 2] {
    let mut taps = [0_i16; MAX_TAPS * 2];

    let dist = distance(x, y, z);
    let tap = calculate_tap_value(dist, magic_scaling_coeff);

    taps[0] = (tap as i16).clamp(TAP_VALUE_MIN, TAP_VALUE_MAX);
    taps
}

fn calculate_tap_value(dist: f32, magic_scaling_coeff: f32) -> f32 {
    if dist == 0. {
        TAP_VALUE_NO_LOSS
    } else {
        (TAP_VALUE_NO_LOSS / (4. * PI * (dist / LAMBDA))) * magic_scaling_coeff
    }
}

fn calculate_taps_two_ray(
    x: f32,
    y: f32,
    z: f32,
    sample_rate: f32,
    magic_scaling_coeff: f32,
) -> [i16; MAX_TAPS * 2] {
    let delay_per_tap = 1. / sample_rate;

    let mut taps = [0_i16; MAX_TAPS * 2];

    let d_los = distance(x, y, z);
    let d_nlos = distance(x, y, z + 2. * STATION_Z);
    let delta_d = d_nlos - d_los;
    let delta_t = delta_d / SPEED_OF_LIGHT;
    let tap_index_second_ray = (delta_t / delay_per_tap).floor() as usize;

    // fit empirically
    // let tap_value_no_loss = (2.0_f32.powi(15) - 1.) / 132.71873 * TAP_VALUE_ONE_METER;

    let tap_los = calculate_tap_value(d_los, magic_scaling_coeff);

    let tap_nlos = calculate_tap_value(d_nlos, magic_scaling_coeff);
    let phi = 2. * PI * ((d_nlos - d_los) / LAMBDA);
    let phase_nlos = (Complex::new(0., 1.) * phi).exp();

    let tap_nlos = tap_nlos * phase_nlos;

    // info!("second ray phase: {}, delta_d: {}, delta_t: {}, d_per_tap: {}, index: {}", phase_nlos, delta_d, delta_t, delay_per_tap, tap_index_second_ray);

    taps[0] = (tap_los as i16).clamp(TAP_VALUE_MIN, TAP_VALUE_MAX);
    // taps[MAX_TAPS] = 0_i16;
    if tap_index_second_ray < MAX_TAPS && tap_index_second_ray > 0 {
        taps[0 + tap_index_second_ray] = (tap_nlos.re as i16).clamp(TAP_VALUE_MIN, TAP_VALUE_MAX);
        taps[MAX_TAPS + tap_index_second_ray] =
            (tap_nlos.im as i16).clamp(TAP_VALUE_MIN, TAP_VALUE_MAX);
    } else if tap_index_second_ray == 0 {
        let tap_los_combined = Complex::new(tap_los, 0.) + tap_nlos;
        taps[0] = (tap_los_combined.re as i16).clamp(TAP_VALUE_MIN, TAP_VALUE_MAX);
        taps[MAX_TAPS] = (tap_los_combined.im as i16).clamp(TAP_VALUE_MIN, TAP_VALUE_MAX);
    }
    taps
}

#[derive(Debug)]
enum Ev {
    ModeManual(f32),
    ModeAutomaticFreeSpace,
    ModeAutomaticFlatEarthTwoRay,
    Value(f32, f32, f32, f32, f32, f32),
}

const NUM_MODES: usize = 3;
const MODEL_INDEX_AUTOMATIC_FREE_SPACE: usize = 0;
const MODEL_INDEX_AUTOMATIC_FLAT_EARTH_TWO_RAY: usize = 1;
const MODEL_INDEX_MANUAL: usize = 2;

fn main() -> Result<()> {
    let args = Args::parse();
    println!("Configuration: {:?}", args);

    let mut fg = Flowgraph::new();

    //FIR
    let mut taps = [Complex32::new(0.0_f32, 0.0_f32); MAX_TAPS];
    taps[0] = Complex32::new(1.0_f32, 0.0_f32);

    let tcp_sink_ag = fg.add_block(TcpSink::new(args.local_tcp_sink_port_ag));
    let tcp_source_ag = fg.add_block(TcpSource::new(args.remote_tcp_sink_address_ag));
    let iq_serializer_ag = fg.add_block(Complex32Serializer::new());
    let iq_deserializer_ag = fg.add_block(Complex32Deserializer::new());
    let rx_noise_ag = fg.add_block(AWGNComplex32::new(args.rx_noise_power));
    let fir_ag_block = FirBuilder::new::<Complex32, Complex32, Complex32, _>(taps);
    let fir_ag_update_taps_input_port_id = fir_ag_block
        .message_input_name_to_id("update_taps")
        .expect("No update_taps port found!");
    let fir_ag = fg.add_block(fir_ag_block);
    let tcp_sink_ga = fg.add_block(TcpSink::new(args.local_tcp_sink_port_ga));
    let tcp_source_ga = fg.add_block(TcpSource::new(args.remote_tcp_sink_address_ga));
    let iq_serializer_ga = fg.add_block(Complex32Serializer::new());
    let iq_deserializer_ga = fg.add_block(Complex32Deserializer::new());
    let rx_noise_ga = fg.add_block(AWGNComplex32::new(args.rx_noise_power));
    let fir_ga_block = FirBuilder::new::<Complex32, Complex32, Complex32, _>(taps);
    let fir_ga_update_taps_input_port_id = fir_ga_block
        .message_input_name_to_id("update_taps")
        .expect("No update_taps port found!");
    let fir_ga = fg.add_block(fir_ga_block);

    // ============================================
    // AG CHANNEL
    // ============================================
    fg.connect_stream(tcp_source_ag, "out", iq_deserializer_ag, "in")?;
    fg.connect_stream(iq_deserializer_ag, "out", rx_noise_ag, "in")?;
    fg.connect_stream(rx_noise_ag, "out", fir_ag, "in")?;
    fg.connect_stream(fir_ag, "out", iq_serializer_ag, "in")?;
    fg.connect_stream(iq_serializer_ag, "out", tcp_sink_ag, "in")?;

    // ============================================
    // GA CHANNEL
    // ============================================
    fg.connect_stream(tcp_source_ga, "out", iq_deserializer_ga, "in")?;
    fg.connect_stream(iq_deserializer_ga, "out", rx_noise_ga, "in")?;
    fg.connect_stream(rx_noise_ga, "out", fir_ga, "in")?;
    fg.connect_stream(fir_ga, "out", iq_serializer_ga, "in")?;
    fg.connect_stream(iq_serializer_ga, "out", tcp_sink_ga, "in")?;

    // ============================================
    // RUNTIME
    // ============================================
    let rt = Runtime::new();
    let (_fg, mut handle) = block_on(rt.start(fg));
    let mut input_handle = handle.clone();

    // ============================================
    // CHANEM
    // ============================================
    let mut magic_scaling_coeff: f32 = MAGIC_SCALING_COEFF;

    let (tx, mut rx) = unbounded_channel();
    let my_tx = tx.clone();
    let my_tx_1 = tx.clone();

    let (to_gui_udp_handler_tx, mut to_gui_udp_handler_rx) = unbounded_channel();
    let to_gui_udp_handler_tx_1 = to_gui_udp_handler_tx.clone();
    let to_gui_udp_handler_tx_2 = to_gui_udp_handler_tx.clone();

    let (mode_channel_gui_to_gamepad_tx, mode_channel_gui_to_gamepad_rx) =
        watch::channel(MODEL_INDEX_AUTOMATIC_FREE_SPACE);

    std::thread::spawn(move || {
        let mut current_value = 50.0_f32;
        let mut gilrs = Gilrs::new().unwrap();
        let gamepad = gilrs.gamepads().next().map(|(_, b)| b);
        if let Some(pad) = gamepad {
            info!("{} is {:?}", pad.name(), pad.power_info());
            loop {
                while let Some(Event { event, .. }) = gilrs.next_event() {
                    let mut pl_model_index = *mode_channel_gui_to_gamepad_rx.borrow();
                    let mut send = false;
                    let mut send_control_event = false;
                    let mut control_event = b"E00";
                    if matches!(event, EventType::ButtonReleased(Button::East, _)) {
                        pl_model_index += 1;
                        pl_model_index = pl_model_index % NUM_MODES;
                        if pl_model_index == MODEL_INDEX_AUTOMATIC_FREE_SPACE {
                            info!("mode automatic - Free-Space PL");
                            my_tx.send(Ev::ModeAutomaticFreeSpace).unwrap();
                        } else if pl_model_index == MODEL_INDEX_AUTOMATIC_FLAT_EARTH_TWO_RAY {
                            info!("mode automatic - Flat-Earth Two-Ray PL");
                            my_tx.send(Ev::ModeAutomaticFlatEarthTwoRay).unwrap();
                        } else {
                            info!("mode manual - {}dB", current_value);
                            my_tx.send(Ev::ModeManual(current_value)).unwrap();
                        }
                        send = true;
                    } else if matches!(event, EventType::ButtonReleased(Button::DPadDown, _)) {
                        if pl_model_index == MODEL_INDEX_MANUAL {
                            current_value += 5.0;
                            current_value = current_value.clamp(0.0, 120.0);
                            info!("mode manual - {}dB", current_value);
                            my_tx.send(Ev::ModeManual(current_value)).unwrap();
                        }
                        send = true;
                    } else if matches!(event, EventType::ButtonReleased(Button::DPadUp, _)) {
                        if pl_model_index == MODEL_INDEX_MANUAL {
                            current_value -= 5.0;
                            current_value = current_value.clamp(0.0, 120.0);
                            info!("mode manual - {}dB", current_value);
                            my_tx.send(Ev::ModeManual(current_value)).unwrap();
                        }
                        send = true;
                    } else if matches!(event, EventType::ButtonPressed(Button::RightTrigger2, _)) {
                        send_control_event = true;
                        control_event = b"ETR";
                    } else if matches!(event, EventType::ButtonPressed(Button::LeftTrigger2, _)) {
                        send_control_event = true;
                        control_event = b"ETL";
                    } else if matches!(event, EventType::ButtonReleased(Button::West, _)) {
                        send_control_event = true;
                        control_event = b"EAW";
                    } else if matches!(event, EventType::ButtonReleased(Button::South, _)) {
                        send_control_event = true;
                        control_event = b"EAS";
                    } else if matches!(event, EventType::ButtonReleased(Button::North, _)) {
                        send_control_event = true;
                        control_event = b"EAN";
                    }
                    if send {
                        let mut send_buf = current_value.to_be_bytes().to_vec();
                        send_buf.insert(0_usize, pl_model_index as u8);
                        // prepend 'M' as message type to distinguish between [P]osition, [T]aps, and [M]ode
                        send_buf.insert(0_usize, b'M');
                        to_gui_udp_handler_tx.send(send_buf).unwrap();
                    }
                    if send_control_event {
                        to_gui_udp_handler_tx.send(control_event.to_vec()).unwrap();
                    }
                }
            }
        }
    });

    rt.spawn_background(async move {
        info!(
            "spawning position update receiver, listening on port {}",
            args.local_udp_port
        );
        let sock = UdpSocket::bind(format!("0.0.0.0:{}", args.local_udp_port))
            .await
            .unwrap();
        let mut buf = [0; 2048];
        loop {
            let (len, addr) = sock.recv_from(&mut buf).await.unwrap();
            debug!("{:?} bytes received from {:?}", len, addr);

            if len == 24 {
                let x = f32::from_be_bytes(buf[0..4].try_into().unwrap());
                let y = f32::from_be_bytes(buf[4..8].try_into().unwrap());
                let z = f32::from_be_bytes(buf[8..12].try_into().unwrap());
                let r_rad = f32::from_be_bytes(buf[12..16].try_into().unwrap());
                let p_rad = f32::from_be_bytes(buf[16..20].try_into().unwrap());
                let y_rad = f32::from_be_bytes(buf[20..24].try_into().unwrap());

                if let Err(e) = tx.send(Ev::Value(x, y, z, r_rad, p_rad, y_rad)) {
                    warn!("could not send position update to gu. ({:?})", e)
                }
                debug!(
                    "received ([{}, {}, {}], [{}, {}, {}])",
                    x, y, z, r_rad, p_rad, y_rad
                );

                let mut send_buf = buf.to_vec();
                // prepend 'P' as message type to distinguish between [P]osition, [T]aps, and [M]ode
                send_buf.insert(0_usize, b'P');
                to_gui_udp_handler_tx_1.send(send_buf).unwrap();
            } else {
                // erroneous message contains: b'PowerFolder node: [1337]-[AUTJpBd5EcTPnEtSPDkZ]\x00'
                // some external program (PowerFolder, probably connected to HessenBox on some PC in the local network) also uses port 1337 -> ignore this specific message
                // there might still arrive other malformed packages -> log for further inspection
                let known_malformed_msg_prefix: [u8; 24] = [
                    80, 111, 119, 101, 114, 70, 111, 108, 100, 101, 114, 32, 110, 111, 100, 101,
                    58, 32, 91, 49, 51, 51, 55, 93,
                ];
                if len > 24 && buf[..24] == known_malformed_msg_prefix {
                } else {
                    info!("WARNING 001: received {:?}", &buf);
                }
            }
        }
    });

    // udp receiver from gui
    rt.spawn_background(async move {
        let sock = UdpSocket::bind(format!("0.0.0.0:{}", args.model_selection_udp_port))
            .await
            .unwrap();
        let mut buf = [0; 1024];
        loop {
            let (len, addr) = sock.recv_from(&mut buf).await.unwrap();
            debug!("{:?} bytes received from {:?}", len, addr);

            if len == 1 {
                // let mut received = std::str::from_utf8(&buf[0..1]).unwrap().trim();
                // let new_pl_model_index = received.parse::<usize>().unwrap();
                let new_pl_model_index = buf[0] as usize;
                if new_pl_model_index == MODEL_INDEX_AUTOMATIC_FREE_SPACE {
                    info!("mode automatic - Free-Space PL");
                    my_tx_1.send(Ev::ModeAutomaticFreeSpace).unwrap();
                } else if new_pl_model_index == MODEL_INDEX_AUTOMATIC_FLAT_EARTH_TWO_RAY {
                    info!("mode automatic - Flat-Earth Two-Ray PL");
                    my_tx_1.send(Ev::ModeAutomaticFlatEarthTwoRay).unwrap();
                } else {
                    info!("mode manual {}", -1.);
                    my_tx_1.send(Ev::ModeManual(-1.)).unwrap();
                }
                info!("received new pl_model_index: {}", new_pl_model_index);
            } else if len == 4 {
                magic_scaling_coeff = f32::from_be_bytes(buf[0..4].try_into().unwrap());
                info!(
                    "received new magic scaling coefficient: {}",
                    magic_scaling_coeff
                );
            } else {
                warn!("received invalid data from GUI: was not of length 1 (u8) or 4 (f32).")
            }
        }
    });

    // udp sender to gui
    rt.spawn_background(async move {
        let sock_tx_to_gui = UdpSocket::bind("0.0.0.0:0").await.unwrap();
        sock_tx_to_gui
            // forward to Host (has .1 address of every docker compose network)
            .connect("172.18.0.1:1342")
            .await
            .unwrap();
        let mut to_gui_udp_handler_rx = to_gui_udp_handler_rx;
        loop {
            if let Some(payload) = to_gui_udp_handler_rx.recv().await {
                match sock_tx_to_gui.send(&payload).await {
                    Ok(l) => {
                        debug!("success sending to GUI.")
                    }
                    Err(e) => {
                        warn!("error sending position update to GUI ({:?})", e);
                    }
                };
            }
        }
    });

    let mut pl_model_index = MODEL_INDEX_AUTOMATIC_FREE_SPACE;
    let mut last_manual = 50.0_f32;
    rt.spawn_background(async move {
        loop {
            let mut send = false;
            let mut taps_tmp = [0_i16; MAX_TAPS * 2];
            if let Some(e) = rx.recv().await {
                match e {
                    Ev::ModeAutomaticFreeSpace => {
                        pl_model_index = MODEL_INDEX_AUTOMATIC_FREE_SPACE;
                        if let Err(e) =
                            mode_channel_gui_to_gamepad_tx.send(MODEL_INDEX_AUTOMATIC_FREE_SPACE)
                        {
                            warn!("error sending PL model index to gui ({:?})", e);
                        }
                    }
                    Ev::ModeAutomaticFlatEarthTwoRay => {
                        pl_model_index = MODEL_INDEX_AUTOMATIC_FLAT_EARTH_TWO_RAY;
                        if let Err(e) = mode_channel_gui_to_gamepad_tx
                            .send(MODEL_INDEX_AUTOMATIC_FLAT_EARTH_TWO_RAY)
                        {
                            warn!("error sending PL model index to gui ({:?})", e);
                        }
                    }
                    Ev::ModeManual(v) => {
                        pl_model_index = MODEL_INDEX_MANUAL;
                        if let Err(e) = mode_channel_gui_to_gamepad_tx.send(MODEL_INDEX_MANUAL) {
                            warn!("error sending PL model index to gui ({:?})", e);
                        }
                        if v >= 0. {
                            last_manual = v;
                        }
                        taps.fill(Complex32::new(0.0_f32, 0.0_f32));
                        // taps.fill(0.0_f32);
                        taps_tmp.fill(0_i16);
                        let tap = TAP_VALUE_NO_LOSS / 10.0_f32.powf(last_manual / 20.0_f32)
                            * magic_scaling_coeff;
                        let tap = (tap as i16).clamp(TAP_VALUE_MIN, TAP_VALUE_MAX);
                        taps_tmp[0] = tap;
                        let tap = tap as f32 / TAP_VALUE_MAX as f32;
                        // taps[0] = Complex32::new(tap, 0_f32);
                        taps[MAX_TAPS - 1] = Complex32::new(tap, 0_f32);
                        send = true;
                    }
                    Ev::Value(x, y, z, _r_rad, _p_rad, _y_rad) => {
                        if pl_model_index == 1 {
                            taps_tmp = calculate_taps_two_ray(
                                x,
                                y,
                                z,
                                args.sample_rate as f32,
                                magic_scaling_coeff,
                            );
                        }
                        // else if pl_model_index == 2 {
                        //     taps = calculate_taps_two_segment_log_dist(x, y, z, r_rad, p_rad, y_rad);
                        // }
                        else if pl_model_index == 0 {
                            taps_tmp = calculate_taps_freespace(x, y, z, magic_scaling_coeff);
                            send = true;
                        }
                        for i in 0..MAX_TAPS {
                            // taps[i] = Complex32::new(taps_tmp[i] as f32 / TAP_VALUE_MAX as f32, taps_tmp[MAX_TAPS + i] as f32 / TAP_VALUE_MAX as f32);
                            taps[MAX_TAPS - 1 - i] = Complex32::new(
                                taps_tmp[i] as f32 / TAP_VALUE_MAX as f32,
                                taps_tmp[MAX_TAPS + i] as f32 / TAP_VALUE_MAX as f32,
                            );
                        }
                        send = true;
                    }
                }

                if send {
                    input_handle
                        .call(
                            fir_ag,
                            fir_ag_update_taps_input_port_id,
                            Pmt::Any(Box::new(taps.clone())),
                        )
                        .await;
                    input_handle
                        .call(
                            fir_ga,
                            fir_ga_update_taps_input_port_id,
                            Pmt::Any(Box::new(taps.clone())),
                        )
                        .await;
                    // fir_ga_block.core.taps = taps;
                    let mut send_buf = taps_tmp
                        .iter()
                        .flat_map(|v| v.to_be_bytes())
                        .collect::<Vec<u8>>();
                    // prepend 'T' as message type to distinguish between [P]osition, [T]aps, and [M]ode
                    send_buf.insert(0_usize, b'T');
                    if let Err(e) = to_gui_udp_handler_tx_2.send(send_buf.clone()) {
                        warn!("error sending Filter Taps to gui ({:?})", e);
                    }
                    debug!("sent message to handler: {:?}", send_buf);
                }
            }
        }
    });
    loop {
        sleep(Duration::from_secs(5));
    }
}

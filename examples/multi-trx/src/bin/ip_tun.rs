use async_process::Command;
use clap::Parser;
use forky_tun::{self, Configuration};
use std::collections::HashMap;
// use std::thread::sleep;
use std::time::Duration;
// use futures::StreamExt;
// use futures::sink::SinkExt;
use std::net::Ipv4Addr;
use serde::{Deserialize, Serialize};
use tokio;
// use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
// use packet::ip::Packet;
// use futuresdr::blocks::soapy::SoapyDevSpec::Dev;
use futuresdr::anyhow::Result;
// use futuresdr::async_io;
use futuresdr::async_io::block_on;
use futuresdr::async_io::Timer;
// use futuresdr::async_net::SocketAddr;
use futuresdr::async_net::UdpSocket;
use futuresdr::blocks::Combine;
use futuresdr::blocks::Fft;
use futuresdr::blocks::FftDirection;
use futuresdr::blocks::MessagePipe;
use futuresdr::blocks::Selector;
use futuresdr::blocks::SelectorDropPolicy as DropPolicy;
use futuresdr::blocks::{Apply, NullSink};
// use futuresdr::blocks::SoapySinkBuilder;
// use futuresdr::blocks::SoapySourceBuilder;
use futuresdr::blocks::seify::SinkBuilder;
use futuresdr::blocks::seify::SourceBuilder;
//use futuresdr::blocks::WebsocketSinkBuilder;
//use futuresdr::blocks::WebsocketSinkMode;
use futuresdr::futures::channel::mpsc;
// use futuresdr::futures::channel::oneshot;
use futuresdr::futures::StreamExt;
use futuresdr::log::debug;
use futuresdr::log::info;
use futuresdr::log::warn;
use futuresdr::macros::connect;
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::buffer::circular::Circular;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::FlowgraphHandle;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::Runtime;
use seify::Device;
// use futuresdr::soapysdr::Device;
use seify::Direction::{Rx, Tx};
// use futuresdr::soapysdr::Direction::{Rx, Tx};

use multitrx::MessageSelector;

use multitrx::MetricsReporter;
use util_blocks::IPDSCPRewriter;

use futuresdr::blocks::Delay as WlanDelay;
use wlan::fft_tag_propagation as wlan_fft_tag_propagation;
use wlan::parse_channel as wlan_parse_channel;
use wlan::Decoder as WlanDecoder;
use wlan::MAX_PAYLOAD_SIZE;
// use wlan::Encoder as WlanEncoder;
use multitrx::Encoder as WlanEncoder;
//use wlan::FftShift;
use wlan::FrameEqualizer as WlanFrameEqualizer;
//use wlan::Keep1InN;
use wlan::Mac as WlanMac;
use wlan::Mapper as WlanMapper;
use wlan::Mcs as WlanMcs;
use wlan::MovingAverage as WlanMovingAverage;
use wlan::Prefix as WlanPrefix;
use wlan::SyncLong as WlanSyncLong;
use wlan::SyncShort as WlanSyncShort;
use wlan::MAX_SYM;

use lora::Decoder;
use lora::Deinterleaver;
use lora::FftDemod;
use lora::FrameSync;
use lora::GrayMapping;
use lora::HammingDec;
use lora::HeaderDecoder;
use lora::HeaderMode;
use lora::{AddCrc, GrayDemap, HammingEnc, Header, Interleaver, Modulate};
use multitrx::Whitening;

// use zigbee::modulator as zigbee_modulator;
// use zigbee::parse_channel as zigbee_parse_channel;
// use zigbee::IqDelay as ZigbeeIqDelay;
// // use zigbee::Mac as ZigbeeMac;
// use multitrx::ZigbeeMac;
// use zigbee::ClockRecoveryMm as ZigbeeClockRecoveryMm;
// use zigbee::Decoder as ZigbeeDecoder;

// const PAD_FRONT: usize = 10000;
// const PAD_TAIL: usize = 10000;
// #[derive(Debug, Deserialize)]
// struct DstPortPriorityMap {
//     scr_port_mapping: HashMap<u16, usize>,
// }
//
//
// fn parse_flow_priority_json(filepath: &str) -> Result<HashMap<u16, DstPortPriorityMap>, String> {
//     if let Ok(priority_file_as_string) = fs::read_to_string(filepath) {
//         let data: HashMap<u16, DstPortPriorityMap> = serde_json::from_str(&priority_file_as_string).unwrap();
//         return Ok(data);
//     }
//     else {
//         let dummy_map: HashMap<u16, DstPortPriorityMap> = HashMap::new();
//         warn!("FLAG 6342");
//         return Ok(dummy_map);
//     }
// }

#[derive(Parser, Debug)]
#[clap(version)]
struct Args {
    /// RX Antenna
    #[clap(long)]
    rx_antenna: Option<String>,
    /// TX Antenna
    #[clap(long)]
    tx_antenna: Option<String>,
    /// Soapy device Filter
    #[clap(long)]
    device_filter: Option<String>,
    /// Zigbee RX Gain
    #[clap(long, default_value_t = 50.0)]
    zigbee_rx_gain: f64,
    /// Zigbee TX Gain
    #[clap(long, default_value_t = 50.0)]
    zigbee_tx_gain: f64,
    /// Zigbee RX Channel
    #[clap(id = "zigbee-rx-channel", long)]
    zigbee_rx_channel: Option<f64>,
    /// Zigbee TX Channel
    #[clap(id = "zigbee-tx-channel", long)]
    zigbee_tx_channel: Option<f64>,
    /// Zigbee Sample Rate
    #[clap(long, default_value_t = 4e6)]
    zigbee_sample_rate: f64,
    /// Zigbee TX/RX Center Frequency
    #[clap(long, default_value_t = 2.45e9)]
    zigbee_center_freq: f64,
    /// Zigbee RX Frequency Offset
    #[clap(long, default_value_t = 0.0)]
    zigbee_rx_freq_offset: f64,
    /// Zigbee TX Frequency Offset
    #[clap(long, default_value_t = 0.0)]
    zigbee_tx_freq_offset: f64,
    /// WLAN RX Gain
    #[clap(long, default_value_t = 40.0)]
    wlan_rx_gain: f64,
    /// WLAN TX Gain
    #[clap(long, default_value_t = 40.0)]
    wlan_tx_gain: f64,
    /// WLAN Sample Rate
    #[clap(long, default_value_t = 20e6)]
    wlan_sample_rate: f64,
    /// WLAN RX Channel Number
    #[clap(long, value_parser = wlan_parse_channel)]
    wlan_rx_channel: Option<f64>,
    /// WLAN TX Channel Number
    #[clap(long, value_parser = wlan_parse_channel)]
    wlan_tx_channel: Option<f64>,
    /// WLAN TX/RX Center Frequency
    #[clap(long, default_value_t = 2.45e9)]
    wlan_center_freq: f64,
    /// Soapy RX Frequency Offset
    #[clap(long, default_value_t = 0.0)]
    wlan_rx_freq_offset: f64,
    /// Soapy TX Frequency Offset
    #[clap(long, default_value_t = 0.0)]
    wlan_tx_freq_offset: f64,
    /// Soapy RX Channel
    #[clap(long, default_value_t = 0)]
    soapy_rx_channel: usize,
    /// Soapy TX Channel
    #[clap(long, default_value_t = 0)]
    soapy_tx_channel: usize,
    /// TX MCS
    #[clap(long, value_parser = WlanMcs::parse, default_value = "qpsk12")]
    wlan_mcs: WlanMcs,
    /// padding front and back
    #[clap(long, default_value_t = 10000)]
    wlan_pad_len: usize,
    /// local IP to bind to
    #[clap(long, value_parser, default_value = "0.0.0.0")]
    local_ip: String,
    /// remote IP to connect to
    #[clap(long, value_parser)]
    remote_ip: String,
    /// local IP to bind to
    #[clap(long, value_parser, default_value = "172.18.0.1:1340")]
    metrics_reporting_socket: String,
    /// local UDP port to receive messages to send
    #[clap(long, value_parser, default_value = "1341")]
    protocol_switching_ctrl_port: u32,
    /// send periodic messages for testing
    #[clap(long, value_parser)]
    tx_interval: Option<f32>,
    /// Stream Spectrum data at ws://0.0.0.0:9001
    #[clap(long, value_parser)]
    spectrum: bool,
    /// Drop policy to apply on the selector.
    #[clap(short, long, default_value = "none")]
    drop_policy: DropPolicy,
    /// Path to JSON mapping ports to ip dscp priority values to override specific flow priorities
    // #[clap(long, value_parser = parse_flow_priority_json, default_value = "")]
    #[clap(long, value_parser)]
    flow_priority_file: String,
}

#[derive(Serialize, Deserialize)]
struct PhyUpdate {
    phy: usize,
    center_freq: f64,
    freq_offset_source: f64,
    freq_offset_sink: f64,
    gain_rx: f64,
    gain_tx: f64,
    sample_rate: f64,
}

const LORA_SF: usize = 7;

// const LARGE_ENOUGH_BUFFER_SIZE_FOR_AARONIA_BURST: usize = 1122580;  // TODO maybe this
// const LARGE_ENOUGH_BUFFER_SIZE_FOR_AARONIA_BURST: usize = 8 * MAX_ENCODED_BITS; // TODO maybe this
//                                                                                 // const LARGE_ENOUGH_BUFFER_SIZE_FOR_LORA_FRAME: usize = 8192 * 4 * 8 * 16;
const LARGE_ENOUGH_BUFFER_SIZE_FOR_LORA_FRAME: usize = (1 << LORA_SF) * 256 * 4 * 4;
const LARGE_ENOUGH_BUFFER_SIZE_FOR_AARONIA_BURST: usize = LARGE_ENOUGH_BUFFER_SIZE_FOR_LORA_FRAME;
// const LARGE_ENOUGH_BUFFER_SIZE_FOR_LORA_FRAME: usize =
//     LARGE_ENOUGH_BUFFER_SIZE_FOR_AARONIA_BURST * 16; // 8192 * 4 * 16; // sufficient for SF8
const LORA_MAX_PAYLOAD_SIZE: usize = 255 - 4; // 256 bytes max frame size - 4 bytes TUN metadata
const LORA_MAX_FRAME_SIZE: usize = 2 * (255 + 16);  // samples

const DSCP_EF: u8 = 0b101110 << 2;
const NUM_PROTOCOLS: usize = 2;
const PROTOCOL_INDEX_WIFI: usize = 0;
const PROTOCOL_INDEX_ZIGBEE: usize = 1;
static MTU_VALUES: [usize; NUM_PROTOCOLS] = [
    MAX_PAYLOAD_SIZE, // WiFi
    // 256 - 4 - 5 - 2, // Zigbee: 256 bytes max frame size - 4 bytes TUN metadata - 5 bytes mac header - 2 bytes mac footer (checksum)
    LORA_MAX_PAYLOAD_SIZE,
];
// use phf::phf_map;
// static FLOW_PRIORITY_MAP: phf::Map<u16, u8> = phf_map! {
//     14550 => DSCP_EF,
//     18570 => DSCP_EF,
//     10317 => DSCP_EF,
//     10318 => DSCP_EF,
// };

fn main() -> Result<()> {
    let args = Args::parse();
    println!("Configuration: {:?}", args);

    let flow_priority_map: HashMap<u16, u8> = HashMap::from([
        (14550, DSCP_EF),
        (18570, DSCP_EF),
        (10317, DSCP_EF), // https://gazebosim.org/api/transport/11.0/envvars.html
        (10318, DSCP_EF),
    ]); // TODO

    let mut size = 4096;
    let prefix_in_size = loop {
        if size / 8 >= MAX_SYM * 64 {
            break size;
        }
        size += 4096
    };
    let mut size = 4096;
    let prefix_out_size = loop {
        if size / 8 >= args.wlan_pad_len + std::cmp::max(args.wlan_pad_len, 1) + 320 + MAX_SYM * 80
        {
            break size;
        }
        size += 4096
    };

    let rx_freq = [args.wlan_rx_channel, args.zigbee_rx_channel];
    let tx_freq = [args.wlan_tx_channel, args.zigbee_tx_channel];
    let mut center_freq = [args.wlan_center_freq, args.zigbee_center_freq];
    let mut rx_freq_offset = [args.wlan_rx_freq_offset, args.zigbee_rx_freq_offset];
    let mut tx_freq_offset = [args.wlan_tx_freq_offset, args.zigbee_tx_freq_offset];
    let mut rx_gain = [args.wlan_rx_gain, args.zigbee_rx_gain];
    let mut tx_gain = [args.wlan_tx_gain, args.zigbee_tx_gain];
    let mut sample_rate = [args.wlan_sample_rate, args.zigbee_sample_rate];

    let mut bw_changed_handler_lora: Vec<(usize, usize)> = vec![];

    let mut fg = Flowgraph::new();

    // ==========================================
    // Device, Source, and Sink
    // ==========================================

    let filter = args.device_filter.unwrap_or_else(|| "".to_string());
    let is_soapy_dev = filter.clone().contains("driver=soapy");
    println!("is_soapy_dev: {}", is_soapy_dev);
    let seify_dev = Device::from_args(&*filter).unwrap();

    seify_dev
        .set_sample_rate(Rx, args.soapy_rx_channel, sample_rate[0])
        .unwrap();
    seify_dev
        .set_sample_rate(Tx, args.soapy_tx_channel, sample_rate[0])
        .unwrap();
    // TODO add to seify and wrap with has_dc_offset_mode(), defaulting to false for non-soapy drivers?
    // seify_dev
    //     .set_dc_offset_mode(Tx, args.soapy_tx_channel, true)
    //     .unwrap();
    // seify_dev
    //     .set_dc_offset_mode(Rx, args.soapy_rx_channel, true)
    //     .unwrap();

    // set tx and rx frequencies
    if let (Some(tx_frequency_from_channel), Some(rx_frequency_from_channel)) =
        (tx_freq[0], rx_freq[0])
    {
        // if channel has been provided, use channel center frequency from lookup-table
        seify_dev
            .set_frequency(Tx, args.soapy_tx_channel, tx_frequency_from_channel)
            .unwrap();
        seify_dev
            .set_frequency(Rx, args.soapy_rx_channel, rx_frequency_from_channel)
            .unwrap();
    } else if is_soapy_dev {
        info!("setting soapy frequencies");
        // else use specified center frequency and offset
        seify_dev
            .set_component_frequency(Tx, args.soapy_tx_channel, "RF", center_freq[0])
            .unwrap();
        seify_dev
            .set_component_frequency(Tx, args.soapy_tx_channel, "BB", tx_freq_offset[0])
            .unwrap();
        seify_dev
            .set_component_frequency(Rx, args.soapy_rx_channel, "RF", center_freq[0])
            .unwrap();
        seify_dev
            .set_component_frequency(Rx, args.soapy_rx_channel, "BB", -rx_freq_offset[0])
            .unwrap();
    } else {
        // is aaronia device, no offset for TX and only one real center freq, tx center freq has to be set as center_freq+offset; also other component names

        info!("setting aaronia frequencies");
        seify_dev
            .set_component_frequency(
                Tx,
                args.soapy_tx_channel,
                "RF",
                center_freq[0] + tx_freq_offset[0],
            )
            .unwrap();
        seify_dev
            .set_component_frequency(Rx, args.soapy_rx_channel, "RF", center_freq[0])
            .unwrap();
        seify_dev
            .set_component_frequency(Rx, args.soapy_rx_channel, "DEMOD", rx_freq_offset[0])
            .unwrap();
    }

    let mut sink = SinkBuilder::new()
        .driver(if is_soapy_dev {
            "soapy"
        } else {
            "aaronia_http"
        })
        .device(seify_dev.clone())
        .gain(tx_gain[0]);
    // .dev_channels(vec![args.soapy_tx_channel]);  // TODO find out how to select channel with seify
    let mut src = SourceBuilder::new()
        .driver(if is_soapy_dev {
            "soapy"
        } else {
            "aaronia_http"
        })
        .device(seify_dev)
        .gain(rx_gain[0]);
    // .dev_channels(vec![args.soapy_rx_channel]);

    if let Some(a) = args.tx_antenna {
        sink = sink.antenna(a);
    }
    if let Some(a) = args.rx_antenna {
        src = src.antenna(a);
    }

    let sink = sink.build().unwrap();
    let src = src.build().unwrap();

    //message handler to change frequency and sample rate during runtime
    let sink_freq_input_port_id = sink
        .message_input_name_to_id("freq")
        .expect("No freq port found!");
    let sink_freq_offset_input_port_id = sink
        .message_input_name_to_id("freq_offset")
        .expect("No freq_offset port found!");
    let sink_sample_rate_input_port_id = sink
        .message_input_name_to_id("sample_rate")
        .expect("No sample_rate port found!");
    let sink_gain_input_port_id = sink
        .message_input_name_to_id("gain")
        .expect("No gain port found!");
    let sink_queue_flush_input_port_id = sink
        .message_input_name_to_id("flush")
        .expect("No flush port found!");
    let sink = fg.add_block(sink);

    let src_freq_input_port_id = src
        .message_input_name_to_id("freq")
        .expect("No freq port found!");
    let src_freq_offset_input_port_id = src
        .message_input_name_to_id("freq_offset")
        .expect("No freq_offset port found!");
    let src_sample_rate_input_port_id = src
        .message_input_name_to_id("sample_rate")
        .expect("No sample_rate port found!");
    let src_gain_input_port_id = src
        .message_input_name_to_id("gain")
        .expect("No gain port found!");
    let src = fg.add_block(src);

    //Soapy Sink Selector
    let sink_selector = Selector::<Complex32, 2, 1>::new(args.drop_policy);
    let input_index_port_id = sink_selector
        .message_input_name_to_id("input_index")
        .expect("No input_index port found!");
    let sink_selector_queue_flush_input_port_id = sink_selector
        .message_input_name_to_id("flush")
        .expect("No flush port found!");
    let sink_selector = fg.add_block(sink_selector);
    fg.connect_stream_with_type(
        sink_selector,
        "out0",
        sink,
        "in",
        Circular::with_size(
            LARGE_ENOUGH_BUFFER_SIZE_FOR_AARONIA_BURST.max(LARGE_ENOUGH_BUFFER_SIZE_FOR_LORA_FRAME),
        ),
    )?;
    // fg.connect_stream(
    //     sink_selector, "out0",
    //     sink, "in",
    // )?;

    //source selector
    let src_selector = Selector::<Complex32, 1, 2>::new(args.drop_policy);
    let output_index_port_id = src_selector
        .message_input_name_to_id("output_index")
        .expect("No output_index port found!");
    let src_selector = fg.add_block(src_selector);
    fg.connect_stream(src, "out", src_selector, "in0")?;

    // ============================================
    // WLAN TRANSMITTER
    // ============================================
    let wlan_mac = fg.add_block(WlanMac::new([0x42; 6], [0x23; 6], [0xff; 6]));
    let wlan_encoder_block = WlanEncoder::new(args.wlan_mcs);
    let wlan_encoder_queue_flush_input_port_id = wlan_encoder_block
        .message_input_name_to_id("flush_queue")
        .expect("No flush_queue port found!");
    let wlan_encoder = fg.add_block(wlan_encoder_block);
    fg.connect_message(wlan_mac, "tx", wlan_encoder, "tx")?;
    let wlan_mapper = fg.add_block(WlanMapper::new());
    fg.connect_stream(wlan_encoder, "out", wlan_mapper, "in")?;
    let mut wlan_fft = Fft::with_options(
        64,
        FftDirection::Inverse,
        true,
        Some((1.0f32 / 52.0).sqrt()),
    );
    wlan_fft.set_tag_propagation(Box::new(wlan_fft_tag_propagation));
    let wlan_fft = fg.add_block(wlan_fft);
    fg.connect_stream(wlan_mapper, "out", wlan_fft, "in")?;
    let wlan_prefix = fg.add_block(WlanPrefix::new(args.wlan_pad_len, args.wlan_pad_len));
    fg.connect_stream_with_type(
        wlan_fft,
        "out",
        wlan_prefix,
        "in",
        Circular::with_size(prefix_in_size),
    )?;

    fg.connect_stream_with_type(
        wlan_prefix,
        "out",
        sink_selector,
        "in0",
        Circular::with_size(prefix_out_size),
    )?;

    // ============================================
    // WLAN RECEIVER
    // ============================================

    let metrics_reporter = fg.add_block(MetricsReporter::new(
        args.metrics_reporting_socket,
        args.local_ip.clone(),
    ));

    let wlan_delay = fg.add_block(WlanDelay::<Complex32>::new(16));
    fg.connect_stream(src_selector, "out0", wlan_delay, "in")?;

    let wlan_complex_to_mag_2 = fg.add_block(Apply::new(|i: &Complex32| i.norm_sqr()));
    let wlan_float_avg = fg.add_block(WlanMovingAverage::<f32>::new(64));
    fg.connect_stream(src_selector, "out0", wlan_complex_to_mag_2, "in")?;
    fg.connect_stream(wlan_complex_to_mag_2, "out", wlan_float_avg, "in")?;

    let wlan_mult_conj = fg.add_block(Combine::new(|a: &Complex32, b: &Complex32| a * b.conj()));
    let wlan_complex_avg = fg.add_block(WlanMovingAverage::<Complex32>::new(48));
    fg.connect_stream(src_selector, "out0", wlan_mult_conj, "in0")?;
    fg.connect_stream(wlan_delay, "out", wlan_mult_conj, "in1")?;
    fg.connect_stream(wlan_mult_conj, "out", wlan_complex_avg, "in")?;

    let wlan_divide_mag = fg.add_block(Combine::new(|a: &Complex32, b: &f32| a.norm() / b));
    fg.connect_stream(wlan_complex_avg, "out", wlan_divide_mag, "in0")?;
    fg.connect_stream(wlan_float_avg, "out", wlan_divide_mag, "in1")?;

    let wlan_sync_short = fg.add_block(WlanSyncShort::new());
    fg.connect_stream(wlan_delay, "out", wlan_sync_short, "in_sig")?;
    fg.connect_stream(wlan_complex_avg, "out", wlan_sync_short, "in_abs")?;
    fg.connect_stream(wlan_divide_mag, "out", wlan_sync_short, "in_cor")?;

    let wlan_sync_long = fg.add_block(WlanSyncLong::new());
    fg.connect_stream(wlan_sync_short, "out", wlan_sync_long, "in")?;

    let mut wlan_fft = Fft::new(64);
    wlan_fft.set_tag_propagation(Box::new(wlan_fft_tag_propagation));
    let wlan_fft = fg.add_block(wlan_fft);
    fg.connect_stream(wlan_sync_long, "out", wlan_fft, "in")?;

    let wlan_frame_equalizer = fg.add_block(WlanFrameEqualizer::new());
    fg.connect_stream(wlan_fft, "out", wlan_frame_equalizer, "in")?;

    let wlan_decoder = fg.add_block(WlanDecoder::new());
    fg.connect_stream(wlan_frame_equalizer, "out", wlan_decoder, "in")?;

    let (wlan_rxed_sender, mut wlan_rxed_frames) = mpsc::channel::<Pmt>(100);
    let wlan_message_pipe = fg.add_block(MessagePipe::new(wlan_rxed_sender));
    fg.connect_message(wlan_decoder, "rx_frames", wlan_message_pipe, "in")?;
    fg.connect_message(wlan_decoder, "rx_frames", metrics_reporter, "rx_wifi_in")?;

    // connect!(
    //         fg,
    //     src_selector.out0 > wlan_delay > wlan_mult_conj.in1;
    //     wlan_mult_conj > wlan_complex_avg > wlan_divide_mag.in0;
    //     src_selector.out0 > wlan_complex_to_mag_2 > wlan_float_avg > wlan_divide_mag.in1;
    //     wlan_divide_mag > wlan_sync_short.in_cor;
    //     wlan_sync_short > wlan_sync_long > wlan_fft > wlan_frame_equalizer > wlan_decoder;
    //     src_selector.out0 > wlan_mult_conj.in0;
    //     wlan_delay > wlan_sync_short.in_sig;
    //     wlan_complex_avg > wlan_sync_short.in_abs;
    //     wlan_decoder.rx_frames | wlan_message_pipe;
    //     wlan_decoder.rx_frames | metrics_reporter.rx_wifi_in;
    //     );

    // // ========================================
    // // ZIGBEE RECEIVER
    // // ========================================
    // let mut last: Complex32 = Complex32::new(0.0, 0.0);
    // let mut iir: f32 = 0.0;
    // let alpha = 0.00016;
    // let avg = fg.add_block(Apply::new(move |i: &Complex32| -> f32 {
    //     let phase = (last.conj() * i).arg();
    //     last = *i;
    //     iir = (1.0 - alpha) * iir + alpha * phase;
    //     phase - iir
    // }));
    //
    // let omega = 2.0;
    // let gain_omega = 0.000225;
    // let mu = 0.5;
    // let gain_mu = 0.03;
    // let omega_relative_limit = 0.0002;
    // let mm = fg.add_block(ZigbeeClockRecoveryMm::new(
    //     omega,
    //     gain_omega,
    //     mu,
    //     gain_mu,
    //     omega_relative_limit,
    // ));
    //
    // let zigbee_decoder = fg.add_block(ZigbeeDecoder::new(6));
    // let zigbee_mac_block = ZigbeeMac::new();

    // ========================================
    // LoRa TRANSMITTER
    // ========================================

    let impl_head = false;
    let has_crc = true;
    let cr = 3;

    let whitening = Whitening::new();
    let header = Header::new(impl_head, has_crc, cr);
    let add_crc = AddCrc::new(has_crc);
    let hamming_enc = HammingEnc::new(cr, LORA_SF);
    let interleaver = Interleaver::new(cr as usize, LORA_SF, 0, args.zigbee_sample_rate as usize);
    let gray_demap = GrayDemap::new(LORA_SF);
    let modulate = Modulate::new(
        LORA_SF,
        args.zigbee_sample_rate as usize,
        args.zigbee_sample_rate as usize,
        // vec![8, 16],
        // vec![42, 12],
        vec![8, 16],
        // 20 * (1 << args.spreading_factor) * args.sample_rate as usize / args.bandwidth,
        20 * (1 << LORA_SF),
        Some(8),
    );
    let modulate_bw_port = modulate
        .message_input_name_to_id("bandwidth")
        .expect("No bandwidth port found!");
    let modulate_sr_port = modulate
        .message_input_name_to_id("sample_rate")
        .expect("No sample_rate port found!");
    let modulate = fg.add_block(modulate);
    bw_changed_handler_lora.push((modulate, modulate_bw_port));
    bw_changed_handler_lora.push((modulate, modulate_sr_port));

    let lora_queue_flush_input_port_id = whitening
        .message_input_name_to_id("flush_queue")
        .expect("No flush_queue port found!");
    let (zigbee_rxed_sender, mut zigbee_rxed_frames) = mpsc::channel::<Pmt>(100);
    let zigbee_message_pipe = MessagePipe::new(zigbee_rxed_sender);

    connect!(
        fg,
        whitening [Circular::with_size(LORA_MAX_FRAME_SIZE)] header [Circular::with_size(LORA_MAX_FRAME_SIZE)]  add_crc [Circular::with_size(LORA_MAX_FRAME_SIZE)]  hamming_enc [Circular::with_size(LORA_MAX_FRAME_SIZE)] interleaver [Circular::with_size(LORA_MAX_FRAME_SIZE)] gray_demap
        [Circular::with_size(LORA_MAX_FRAME_SIZE)]
        // modulate [Circular::with_size(LARGE_ENOUGH_BUFFER_SIZE_FOR_AARONIA_BURST.max(LARGE_ENOUGH_BUFFER_SIZE_FOR_LORA_FRAME))] sink_selector.in1
        modulate [Circular::with_size(LARGE_ENOUGH_BUFFER_SIZE_FOR_AARONIA_BURST.max(LARGE_ENOUGH_BUFFER_SIZE_FOR_LORA_FRAME))] sink_selector.in1;
        whitening.flush_propagate | header.flush_queue;
        header.flush_propagate | add_crc.flush_queue;
        add_crc.flush_propagate | hamming_enc.flush_queue;
        hamming_enc.flush_propagate | interleaver.flush_queue;
        interleaver.flush_propagate | gray_demap.flush_queue;
        gray_demap.flush_propagate | modulate.flush_queue;
    );

    // ========================================
    // LoRa RECEIVER
    // ========================================

    let soft_decoding: bool = false;
    // let downsample = FirBuilder::new_resampling::<Complex32, Complex32>(5, 8);
    let frame_sync = FrameSync::new(
        // center_freq[1] as u32 - rx_freq_offset[1] as u32,  // TODO
        center_freq[1] as u32 - rx_freq_offset[1] as u32,
        args.zigbee_sample_rate as u32,
        LORA_SF,
        false,
        vec![8, 16],
        1,
        None,
    );
    let frame_sync_bw_port = frame_sync
        .message_input_name_to_id("bandwidth")
        .expect("No bandwidth port found!");
    let frame_sync_center_freq_port = frame_sync
        .message_input_name_to_id("center_freq")
        .expect("No bandwidth port found!");
    let frame_sync = fg.add_block(frame_sync);
    bw_changed_handler_lora.push((frame_sync, frame_sync_bw_port));
    let null_sink = NullSink::<f32>::new();
    let fft_demod = FftDemod::new(soft_decoding, true, LORA_SF);
    let gray_mapping = GrayMapping::new(soft_decoding);
    let deinterleaver = Deinterleaver::new(soft_decoding);
    let hamming_dec = HammingDec::new(soft_decoding);
    let header_decoder = HeaderDecoder::new(HeaderMode::Explicit, false);
    let decoder = Decoder::new();

    connect!(
        fg,
        src_selector.out1 [Circular::with_size(LARGE_ENOUGH_BUFFER_SIZE_FOR_LORA_FRAME)]
        // src_selector.out1 >
        frame_sync > fft_demod > gray_mapping > deinterleaver > hamming_dec > header_decoder;
        frame_sync.log_out > null_sink;
        header_decoder.frame_info | frame_sync.frame_info;
        header_decoder | decoder;
        decoder.crc_check | frame_sync.payload_crc_result;
        decoder.data | zigbee_message_pipe;
        decoder.data | metrics_reporter.rx_in;
    );

    // fg.connect_stream(src_selector, "out1", avg, "in")?;
    // fg.connect_stream(avg, "out", mm, "in")?;
    // fg.connect_stream(mm, "out", zigbee_decoder, "in")?;
    // fg.connect_message(zigbee_decoder, "out", zigbee_mac, "rx")?;
    // fg.connect_message(zigbee_mac, "rxed", zigbee_message_pipe, "in")?;
    // fg.connect_message(zigbee_mac, "rxed", metrics_reporter, "rx_in")?;

    // // ========================================
    // // ZIGBEE TRANSMITTER
    // // ========================================
    //
    // //let zigbee_mac = fg.add_block(ZigbeeMac::new());
    // let zigbee_modulator = fg.add_block(zigbee_modulator());
    // let zigbee_iq_delay = fg.add_block(ZigbeeIqDelay::new());
    //
    // fg.connect_stream(zigbee_mac, "out", zigbee_modulator, "in")?;
    // fg.connect_stream(zigbee_modulator, "out", zigbee_iq_delay, "in")?;
    // // fg.connect_stream(zigbee_iq_delay, "out", sink_selector, "in1")?;
    // fg.connect_stream_with_type(
    //     zigbee_iq_delay,
    //     "out",
    //     sink_selector,
    //     "in1",
    //     Circular::with_size(LARGE_ENOUGH_BUFFER_SIZE_FOR_AARONIA_BURST),
    // )?;

    // ========================================
    // MESSAGE INPUT SELECTOR
    // ========================================

    let message_selector = MessageSelector::new();
    let output_selector_port_id = message_selector
        .message_input_name_to_id("output_selector")
        .expect("No output_selector port found!");
    let message_selector = fg.add_block(message_selector);
    fg.connect_message(message_selector, "out0", wlan_mac, "tx")?;
    fg.connect_message(message_selector, "out1", whitening, "tx")?;

    // ========================================
    // FLOW PRIORITY TO IP DSCP MAPPER
    // ========================================

    // let ip_dscp_rewriter = IPDSCPRewriter::new(flow_priority_map);
    let ip_dscp_rewriter = if args.local_ip == "192.168.42.11" {
        IPDSCPRewriter::new(flow_priority_map, Some("172.18.0.13:1337"), (49.872653, 8.635148, 181.0))
    } else {
        IPDSCPRewriter::new(flow_priority_map, None, (0., 0., 0.))
    };
    let fg_tx_port = ip_dscp_rewriter
        .message_input_name_to_id("in")
        .expect("No message_in port found!");
    let ip_dscp_rewriter = fg.add_block(ip_dscp_rewriter);
    fg.connect_message(ip_dscp_rewriter, "out", metrics_reporter, "tx_in")?;
    fg.connect_message(ip_dscp_rewriter, "out", message_selector, "message_in")?;

    // ========================================
    // Spectrum
    // ========================================
    // if args.spectrum {
    //     let snk = fg.add_block(
    //         WebsocketSinkBuilder::<f32>::new(9001)
    //             .mode(WebsocketSinkMode::FixedDropping(2048))
    //             .build(),
    //     );
    //     let fft = fg.add_block(Fft::new(2048));
    //     let shift = fg.add_block(FftShift::new());
    //     let keep = fg.add_block(Keep1InN::new(0.5, 10));
    //     let cpy = fg.add_block(futuresdr::blocks::Copy::<Complex32>::new());

    //     fg.connect_stream(src, "out", cpy, "in")?;
    //     fg.connect_stream(cpy, "out", fft, "in")?;
    //     fg.connect_stream(fft, "out", shift, "in")?;
    //     fg.connect_stream(shift, "out", keep, "in")?;
    //     fg.connect_stream(keep, "out", snk, "in")?;
    // }

    let rt = Runtime::new();
    let (_fg, mut handle): (_, FlowgraphHandle) = block_on(rt.start(fg));
    let mut input_handle: FlowgraphHandle = handle.clone();

    // if tx_interval is set, send messages periodically
    if let Some(tx_interval) = args.tx_interval {
        // let mut seq = 0u64;
        let mut myhandle: FlowgraphHandle = handle.clone();
        rt.spawn_background(async move {
            loop {
                Timer::after(Duration::from_secs_f32(tx_interval)).await;
                let dummy_packet = b"\x00\x00\x00\x00\x45\x00\x00\x34\xaf\xf4\x40\x00\x40\x06\x8c\xcd\x7f\x00\x00\x01\x7f\x00\x00\x01\x99\xe2\x9f\xd7\xa6\x79\xa6\xd9\xc9\x3a\x0c\xb4\x80\x10\x02\x00\xfe\x28\x00\x00\x01\x01\x08\x0a\xc2\x08\x96\x5c\xc2\x08\x96\x2f".to_vec();
                myhandle
                    .call(
                        ip_dscp_rewriter,
                        fg_tx_port,
                        Pmt::Blob(dummy_packet),
                        // Pmt::Blob(format!("FutureSDR {}", seq).as_bytes().to_vec()),
                    )
                    .await
                    .unwrap();
                debug!("sending sample packet.");
                // seq += 1;
            }
        });
    }

    info!(
        "Acting as IP tunnel from {} to {}.",
        args.local_ip.clone(),
        args.remote_ip
    );
    let mut tun_config = Configuration::default();
    tun_config
        .name("chanem")
        .address(args.local_ip.clone())
        .netmask((255, 255, 255, 0))
        .destination(args.remote_ip.clone())
        .queues(1)
        .mtu(MTU_VALUES[0].try_into().unwrap())
        .up();
    #[cfg(target_os = "linux")]
    tun_config.platform(|tun_config| {
        tun_config.packet_information(true);
    });

    let rt_tokio = tokio::runtime::Runtime::new().unwrap();
    let (tx_tun_dev1, mut rx_tun_dev1) = tokio::sync::mpsc::channel(1);
    let _keep_channel_open = tx_tun_dev1.clone();
    // let (tx_tun_dev2, mut rx_tun_dev2) = oneshot::channel::<forky_tun::AsyncQueue>();
    // let (tx_tun_dev3, mut rx_tun_dev3) = oneshot::channel::<forky_tun::AsyncQueue>();
    rt_tokio.spawn(async move {
        let tun_dev = forky_tun::create_as_async(&tun_config)
            .expect("Creating a TUN device requires NET_ADMIN privileges.");
        let mut tun_queues = tun_dev.queues().unwrap();
        let tun_queue1 = tun_queues.remove(0);
        // let tun_queue2 = tun_queues.remove(0);
        // let tun_queue3 = tun_queues.remove(0);
        // println!("{:?}", tun_queue2.get_ref().tun);
        match tx_tun_dev1.send(tun_queue1).await {
            Ok(_) => {}
            Err(_) => panic!("could not send TUN interface handle out of async creation context."),
        }
        // println!("{:?}", tun_queue2.get_ref().as_raw_fd());
        // tx_tun_dev2.send(tun_queue2);
        // tx_tun_dev3.send(tun_queue3);
        println!("TUN setup successful.");
    });

    println!("receiving TUN queue");
    let tun_queue1: std::sync::Arc<forky_tun::AsyncQueue> =
        std::sync::Arc::new(rx_tun_dev1.blocking_recv().unwrap());
    println!("received TUN queue");
    rx_tun_dev1.close();
    let tun_queue2 = tun_queue1.clone();
    let tun_queue3 = tun_queue1.clone();

    let sending_thread = rt.spawn(async move {
        println!("initialized sender.");
        let mut buf = vec![0u8; 1024];
        loop {
            // println!("blub");
            match tun_queue1.recv(&mut buf).await {
                Ok(n) => {
                    // 4 bytes offset due to flag bytes added to the front of each packet by TUN interface
                    // if format!("{}.{}.{}.{}", buf[20], buf[21], buf[22], buf[23]) != remote_ip1 {
                    //     println!("{:?}", buf);
                    //     warn!("received packet with dst_ip not matching {}", remote_ip1);
                    //     continue;  // TODO
                    // }

                    print!("s");
                    handle
                        .call(ip_dscp_rewriter, fg_tx_port, Pmt::Blob(buf[0..n].to_vec()))
                        .await
                        .unwrap();
                }
                Err(_) => break,
            }
        }
    });

    rt.spawn_background(async move {
        println!("initialized WiFi receiver.");
        loop {
            if let Some(p) = wlan_rxed_frames.next().await {
                if let Pmt::Blob(v) = p {
                    // info!("received frame, size {}", v.len() - 24);
                    print!("r");
                    tun_queue2.send(&v[24..].to_vec()).await.unwrap();
                } else {
                    warn!("pmt to tx was not a blob");
                }
            } else {
                warn!("cannot read from MessagePipe receiver");
            }
        }
    });

    rt.spawn_background(async move {
        println!("initialized ZigBee receiver.");
        loop {
            if let Some(p) = zigbee_rxed_frames.next().await {
                if let Pmt::Blob(v) = p {
                    // info!("received Zigbee frame size {}", v.len());
                    print!("r");
                    tun_queue3.send(&v.to_vec()).await.unwrap();
                } else {
                    warn!("pmt to tx was not a blob");
                }
            } else {
                warn!("cannot read from MessagePipe receiver");
            }
        }
    });

    // protocol switching message handler:
    info!(
        "listening for protocol switch on port {}.",
        args.protocol_switching_ctrl_port
    );
    let socket: UdpSocket = block_on(UdpSocket::bind((
        Ipv4Addr::UNSPECIFIED,
        args.protocol_switching_ctrl_port as u16,
    )))
    .unwrap();

    rt.spawn_background(async move {
        let mut current_mtu = MTU_VALUES[0];
        let mut buf = vec![0u8; 1024];
        loop {
            match socket.recv_from(&mut buf).await {
                Ok((n, s)) => {
                    let the_string = std::str::from_utf8(&buf[0..n]).expect("not UTF-8");
                    // println!("received JSON: {}", the_string);
                    let p: PhyUpdate = serde_json::from_str(the_string).expect("received invalid Phy update: {the_string}");
                    println!("received protocol number {} from {:?}", p.phy, s);
                    center_freq[p.phy] = p.center_freq;
                    println!("setting center_freq to {}", p.center_freq);
                    tx_freq_offset[p.phy] = p.freq_offset_sink;
                    println!("setting tx_freq_offset to {}", p.freq_offset_sink);
                    rx_freq_offset[p.phy] = p.freq_offset_source;
                    println!("setting rx_freq_offset to {}", p.freq_offset_source);
                    sample_rate[p.phy] = p.sample_rate;
                    println!("setting sample_rate to {}", p.sample_rate);
                    tx_gain[p.phy] = p.gain_tx;
                    println!("setting tx_gain to {}", p.gain_tx);
                    rx_gain[p.phy] = p.gain_rx;
                    println!("setting rx_gain to {}", p.gain_rx);

                    if p.phy < NUM_PROTOCOLS {
                        let new_mtu = MTU_VALUES[p.phy];
                        println!("setting MTU to {new_mtu}");
                        if new_mtu < current_mtu {
                            // switch to smaller MTU before changing the PHY to avoid dropping packets
                            if let Err(e) = Command::new("sh").arg("-c").arg(format!("ifconfig chanem mtu {} up", new_mtu)).output().await {
                                warn!("could not change MTU of TUN interface. Original error message: {}", e);
                            };
                            current_mtu = new_mtu;
                        }
                        let new_index = p.phy as u32;
                        println!("Setting source index to {}", new_index);
                        // set center freq
                        block_on(
                            input_handle
                                .call(
                                    src,
                                    src_freq_input_port_id,
                                    Pmt::VecPmt(vec![Pmt::F64(center_freq[p.phy]), Pmt::U32(args.soapy_rx_channel as u32)])
                                )
                        ).unwrap();
                        block_on(
                            input_handle
                                .call(
                                    sink,
                                    sink_freq_input_port_id,
                                    Pmt::VecPmt(vec![Pmt::F64(center_freq[p.phy]), Pmt::U32(args.soapy_tx_channel as u32)])
                                )
                        ).unwrap();
                        // set freq offset
                        block_on(
                            input_handle
                                .call(
                                    src,
                                    src_freq_offset_input_port_id,
                                    Pmt::VecPmt(vec![Pmt::F64(rx_freq_offset[new_index as usize]), Pmt::U32(args.soapy_rx_channel as u32)])
                                )
                        ).unwrap();
                        block_on(
                            input_handle
                                .call(
                                    sink,
                                    sink_freq_offset_input_port_id,
                                    Pmt::VecPmt(vec![Pmt::F64(tx_freq_offset[new_index as usize]), Pmt::U32(args.soapy_tx_channel as u32)])
                                )
                        ).unwrap();
                        // update effective center freq in lora frame detector
                        if p.phy == PROTOCOL_INDEX_ZIGBEE {
                            block_on(
                                input_handle
                                    .call(
                                        frame_sync,
                                        frame_sync_center_freq_port,
                                        Pmt::Usize((center_freq[PROTOCOL_INDEX_ZIGBEE] - rx_freq_offset[PROTOCOL_INDEX_ZIGBEE]) as usize),
                                    )
                            ).unwrap();
                        }
                        // set sample rate
                        block_on(
                            input_handle
                                .call(
                                    src,
                                    src_sample_rate_input_port_id,
                                    Pmt::F64(sample_rate[new_index as usize])
                                )
                        ).unwrap();
                        block_on(
                            input_handle
                                .call(
                                    sink,
                                    sink_sample_rate_input_port_id,
                                    Pmt::F64(sample_rate[new_index as usize])
                                )
                        ).unwrap();
                        // set gain
                        block_on(
                            input_handle
                                .call(
                                    src,
                                    src_gain_input_port_id,
                                    Pmt::F64(rx_gain[new_index as usize])
                                )
                        ).unwrap();
                        block_on(
                            input_handle
                                .call(
                                    sink,
                                    sink_gain_input_port_id,
                                    Pmt::F64(tx_gain[new_index as usize])
                                )
                        ).unwrap();
                        // set flowgraph selector indices
                        block_on(
                            input_handle
                                .call(
                                    src_selector,
                                    output_index_port_id,
                                    Pmt::U32(new_index)
                                )
                        ).unwrap();
                        block_on(
                            input_handle
                                .call(
                                    sink_selector,
                                    input_index_port_id,
                                    Pmt::U32(new_index)
                                )
                        ).unwrap();
                        block_on(
                            input_handle
                                .call(
                                    message_selector,
                                    output_selector_port_id,
                                    Pmt::U32(new_index)
                                )
                        ).unwrap();
                        // flush queues
                        block_on(
                            input_handle
                                .call(
                                    sink_selector,
                                    sink_selector_queue_flush_input_port_id,
                                    Pmt::Null
                                )
                        ).unwrap();
                        block_on(
                            input_handle
                                .call(
                                    sink,
                                    sink_queue_flush_input_port_id,
                                    Pmt::Null
                                )
                        ).unwrap();
                        if p.phy == PROTOCOL_INDEX_ZIGBEE {
                            block_on(
                                input_handle
                                    .call(
                                        whitening,
                                        lora_queue_flush_input_port_id,
                                        Pmt::Null,
                                )
                            ).unwrap();
                            // update BW in lora blocks
                            for (block, port) in &bw_changed_handler_lora {
                                block_on(
                                    input_handle
                                        .call(
                                            *block,
                                            *port,
                                            Pmt::Usize(sample_rate[PROTOCOL_INDEX_ZIGBEE] as usize),
                                        )
                                ).unwrap();
                            }
                        }
                        else if p.phy == PROTOCOL_INDEX_WIFI {
                            block_on(
                                input_handle
                                    .call(
                                    wlan_encoder,
                                    wlan_encoder_queue_flush_input_port_id,
                                    Pmt::Null,
                                )
                            ).unwrap();
                        }
                        if new_mtu > current_mtu {
                            // switch to larger MTU after changing the PHY to avoid dropping packets
                            if let Err(e) = Command::new("sh").arg("-c").arg(format!("ifconfig chanem mtu {} up", new_mtu)).output().await {
                                warn!("could not change MTU of TUN interface. Original error message: {}", e);
                            };
                            current_mtu = new_mtu;
                        }
                    }
                    else {
                        println!("Invalid protocol index.")
                    }
                }
                Err(e) => println!("ERROR: {:?}", e),
            }
        }
    });

    // let socket_protocol_num = block_on(UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0_u16))).unwrap();

    // // if this program is running in an interactive terminal
    // if atty::is(atty::Stream::Stdin) {
    //     // Keep asking user for the selection
    //     loop {
    //         println!("Enter a new output index");
    //         // Get input from stdin and remove all whitespace (most importantly '\n' at the end)
    //         let mut input = String::new(); // Input buffer
    //         std::io::stdin()
    //             .read_line(&mut input)
    //             .expect("error: unable to read user input");
    //         input.retain(|c| !c.is_whitespace());
    //
    //         // If the user entered a valid number, set the new frequency, gain and sample rate by sending a message to the `FlowgraphHandle`
    //         block_on(socket_protocol_num.send_to(&input.into_bytes(), format!("{}:{}", args.local_ip, args.protocol_switching_ctrl_port))).unwrap();
    //     }
    // }
    // else {
    println!("running in background, disabling manual protocol selection.");
    // loop {
    //     sleep(Duration::from_secs(5));
    // }
    block_on(sending_thread);
    Ok(())
    // }
}

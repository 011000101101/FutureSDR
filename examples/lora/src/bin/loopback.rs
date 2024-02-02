use chrono::prelude::*;
use clap::Parser;
use futuresdr::anyhow::Result;
use futuresdr::async_io::Timer;
use futuresdr::blocks::seify::SinkBuilder;
use futuresdr::blocks::seify::SourceBuilder;
use futuresdr::blocks::FirBuilder;
use futuresdr::blocks::{BlobToUdp, Split};
use futuresdr::blocks::{FileSink, NullSink};
use futuresdr::log::{debug, info};
use futuresdr::macros::connect;
use futuresdr::runtime::buffer::circular::Circular;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::Runtime;
use lora::Decoder;
use lora::Deinterleaver;
use lora::FftDemod;
use lora::FrameSync;
use lora::GrayMapping;
use lora::HammingDec;
use lora::HeaderDecoder;
use lora::HeaderMode;
use lora::{AddCrc, GrayDemap, HammingEnc, Header, Interleaver, Modulate, Whitening};
use rustfft::num_complex::Complex32;
use seify::Device;
use seify::Direction::{Rx, Tx};
use std::fmt::Debug;
use std::time::Duration;

#[derive(Parser, Debug)]
#[clap(version)]
struct Args {
    /// RX Antenna
    #[clap(long)]
    tx_antenna: Option<String>,
    /// Soapy device Filter
    #[clap(long)]
    device_filter: Option<String>,
    /// Zigbee RX Gain
    #[clap(long, default_value_t = -40.0)]
    tx_gain: f64,
    /// Zigbee RX Gain
    #[clap(long, default_value_t = 20.0)]
    rx_gain: f64,
    /// Zigbee Sample Rate
    #[clap(long, default_value_t = 125e3)]
    sample_rate: f64,
    /// Zigbee TX/RX Center Frequency
    #[clap(long, default_value_t = 868.1e6)]
    center_freq: f64,
    /// Zigbee RX Frequency Offset
    #[clap(long, default_value_t = 0.0)]
    tx_freq_offset: f64,
    /// Soapy RX Channel
    #[clap(long, default_value_t = 0)]
    soapy_tx_channel: usize,
    /// send periodic messages for testing
    #[clap(long, value_parser)]
    tx_interval: Option<f32>,
    /// lora spreading factor
    #[clap(long, default_value_t = 7)]
    spreading_factor: usize,
    /// lora bandwidth
    #[clap(long, default_value_t = 125000)]
    bandwidth: usize,
    /// LoRa Sync Word
    #[clap(long, default_value_t = 0x0816)]
    sync_word: u16,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let rt = Runtime::new();
    let mut fg = Flowgraph::new();

    let filter = args.device_filter.unwrap_or_else(String::new);
    let is_soapy_dev = filter.clone().contains("driver=soapy");
    println!("is_soapy_dev: {}", is_soapy_dev);
    let seify_dev = Device::from_args(&*filter).unwrap();

    seify_dev
        .set_sample_rate(Tx, args.soapy_tx_channel, args.sample_rate)
        .unwrap();

    if is_soapy_dev {
        info!("setting soapy frequencies");
        // else use specified center frequency and offset
        seify_dev
            .set_component_frequency(Rx, args.soapy_tx_channel, "RF", args.center_freq)
            .unwrap();
        seify_dev
            .set_component_frequency(Rx, args.soapy_tx_channel, "BB", args.tx_freq_offset)
            .unwrap();
        seify_dev
            .set_component_frequency(Tx, args.soapy_tx_channel, "RF", args.center_freq)
            .unwrap();
        seify_dev
            .set_component_frequency(Tx, args.soapy_tx_channel, "BB", args.tx_freq_offset)
            .unwrap();
    } else {
        // is aaronia device, no offset for TX and only one real center freq, tx center freq has to be set as center_freq+offset; also other component names
        info!(
            "setting aaronia frequency to {}",
            args.center_freq + args.tx_freq_offset
        );
        seify_dev
            .set_component_frequency(Rx, args.soapy_tx_channel, "RF", args.center_freq)
            .unwrap();
        seify_dev
            .set_component_frequency(Rx, args.soapy_tx_channel, "DEMOD", args.tx_freq_offset)
            .unwrap();
        seify_dev
            .set_component_frequency(
                Tx,
                args.soapy_tx_channel,
                "RF",
                args.center_freq + args.tx_freq_offset,
            )
            .unwrap();
        // seify_dev
        //     .set_component_frequency(Tx, args.soapy_tx_channel, "DEMOD", args.tx_freq_offset)
        //     .unwrap();
    }

    let soft_decoding: bool = false;

    let mut src = SourceBuilder::new()
        .device(seify_dev.clone())
        .sample_rate(args.bandwidth as f64)
        // .sample_rate(200.0e3)
        .gain(args.rx_gain);
    let src = src.build().unwrap();
    // let downsample = FirBuilder::new_resampling::<Complex32, Complex32>(5, 8);
    let split = Split::new(|x: &Complex32| (*x, *x));
    let file_sink = FileSink::<Complex32>::new(format!(
        "/tmp/lora_loopback_rx_{}.bin",
        Local::now().format("%Y-%m-%d_%H-%M")
    ));
    let frame_sync = FrameSync::new(
        args.center_freq as u32,
        args.bandwidth as u32,
        // 200000,
        args.spreading_factor,
        false,
        vec![args.sync_word.into()],
        1,
        None,
    );
    let null_sink = NullSink::<f32>::new();
    let fft_demod = FftDemod::new(soft_decoding, true, args.spreading_factor);
    let gray_mapping = GrayMapping::new(soft_decoding);
    let deinterleaver = Deinterleaver::new(soft_decoding);
    let hamming_dec = HammingDec::new(soft_decoding);
    let header_decoder = HeaderDecoder::new(HeaderMode::Explicit, false);
    let decoder = Decoder::new();
    // let udp_data = BlobToUdp::new("127.0.0.1:55555");
    // let udp_rftap = BlobToUdp::new("127.0.0.1:55556");
    connect!(fg, src
        // > downsample
        > split;
        split.out0 > file_sink;
        split.out1 [Circular::with_size(2 * 4 * 8192 * 4)] frame_sync > fft_demod > gray_mapping > deinterleaver > hamming_dec > header_decoder;
        frame_sync.log_out > null_sink;
        header_decoder.frame_info | frame_sync.frame_info;
        header_decoder | decoder;
        decoder.crc_check | frame_sync.payload_crc_result;
        // decoder.data | udp_data;
        // decoder.rftap | udp_rftap;
    );

    let mut sink = SinkBuilder::new()
        .driver(if is_soapy_dev {
            "soapy"
        } else {
            "aaronia_http"
        })
        .device(seify_dev)
        .gain(args.tx_gain);
    // .dev_channels(vec![args.soapy_rx_channel]);

    if let Some(a) = args.tx_antenna {
        sink = sink.antenna(a);
    }

    let sink = sink.build().unwrap();

    let impl_head = false;
    let has_crc = true;
    let cr = 3;

    let whitening = Whitening::new(false, false);
    let fg_tx_port = whitening
        .message_input_name_to_id("msg")
        .expect("No message_in port found!");
    let header = Header::new(impl_head, has_crc, cr);
    let add_crc = AddCrc::new(has_crc);
    let hamming_enc = HammingEnc::new(cr, args.spreading_factor);
    let interleaver = Interleaver::new(cr as usize, args.spreading_factor, 0, args.bandwidth);
    let gray_demap = GrayDemap::new(args.spreading_factor);
    let modulate = Modulate::new(
        args.spreading_factor,
        args.sample_rate as usize,
        args.bandwidth,
        // vec![8, 16],
        // vec![42, 12],
        vec![args.sync_word.into()],
        20 * (1 << args.spreading_factor) * args.sample_rate as usize / args.bandwidth,
        Some(8),
    );
    connect!(
        fg,
        whitening > header > add_crc > hamming_enc > interleaver > gray_demap
        >
        modulate [Circular::with_size(2 * 4 * 8192 * 4 * 8 * 16)] sink;
    );

    // if tx_interval is set, send messages periodically
    if let Some(tx_interval) = args.tx_interval {
        let (_fg, mut handle) = rt.start_sync(fg);
        rt.block_on(async move {
            let mut counter: usize = 0;
            loop {
                let dummy_packet = format!("hello world! {:02}", counter).to_string();
                // let dummy_packet = "hello world!1".to_string();
                handle
                    .call(whitening, fg_tx_port, Pmt::String(dummy_packet))
                    .await
                    .unwrap();
                debug!("sending sample packet.");
                counter += 1;
                counter %= 100;
                Timer::after(Duration::from_secs_f32(tx_interval)).await;
            }
        });
    } else {
        let _ = rt.run(fg);
    }

    Ok(())
}

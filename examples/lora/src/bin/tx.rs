use clap::Parser;
use futuresdr::anyhow::Result;
use futuresdr::async_io::Timer;
use futuresdr::blocks::seify::SinkBuilder;
use futuresdr::log::{debug, info};
use futuresdr::macros::connect;
use futuresdr::runtime::buffer::circular::Circular;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::Runtime;
use lora::{AddCrc, GrayDemap, HammingEnc, Header, Interleaver, Modulate, Whitening};
use seify::Device;
use seify::Direction::{Rx, Tx};
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
    #[clap(long, default_value_t = 50.0)]
    tx_gain: f64,
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

    let sink = fg.add_block(sink.build().unwrap());

    let impl_head = false;
    let has_crc = true;
    let cr = 3;

    let whitening = Whitening::new(false, false);
    let fg_tx_port = whitening
        .message_input_name_to_id("msg")
        .expect("No message_in port found!");
    let whitening = fg.add_block(whitening);
    let header = fg.add_block(Header::new(impl_head, has_crc, cr));
    let add_crc = fg.add_block(AddCrc::new(has_crc));
    let hamming_enc = fg.add_block(HammingEnc::new(cr, args.spreading_factor));
    let interleaver = fg.add_block(Interleaver::new(
        cr as usize,
        args.spreading_factor,
        0,
        args.bandwidth,
    ));
    let gray_demap = fg.add_block(GrayDemap::new(args.spreading_factor));
    let modulate = fg.add_block(Modulate::new(
        args.spreading_factor,
        args.sample_rate as usize,
        args.bandwidth,
        vec![8, 16],
        // vec![42, 12],
        20 * (1 << args.spreading_factor) * args.sample_rate as usize / args.bandwidth,
        Some(8),
    ));
    //fg.connect_stream_with_type(
    //    modulate,
    //    "out",
    //    sink,
    //    "in",
    // Circular::with_size(2 * 4 * 8192 * 4 * 8),
    //    Circular::with_size(2 * 4 * 8192 * 4 * 8 * 16),
    //)?;
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

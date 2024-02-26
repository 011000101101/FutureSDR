use clap::Parser;
use futuresdr::futures::channel::mpsc;
use futuresdr::futures::StreamExt;
use std::time::Duration;

use futuresdr::anyhow::Result;
use futuresdr::async_io::Timer;
use futuresdr::blocks::seify::SinkBuilder;
use futuresdr::blocks::seify::SourceBuilder;
use futuresdr::blocks::Apply;
use futuresdr::blocks::Combine;
use futuresdr::blocks::Delay;
use futuresdr::blocks::Fft;
use futuresdr::blocks::FftDirection;
use futuresdr::blocks::MessagePipe;
use futuresdr::blocks::WebsocketPmtSink;
use futuresdr::macros::connect;
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::buffer::circular::Circular;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::Runtime;
use futuresdr::seify::Device;

use wlan::fft_tag_propagation;
use wlan::parse_channel;
use wlan::Decoder;
use wlan::Encoder;
use wlan::FrameEqualizer;
use wlan::Mac;
use wlan::Mapper;
use wlan::Mcs;
use wlan::MovingAverage;
use wlan::Prefix;
use wlan::SyncLong;
use wlan::SyncShort;

#[derive(Parser, Debug)]
#[clap(version)]
struct Args {
    /// Soapy device Filter
    #[clap(long)]
    device_filter: Option<String>,
    /// Gain
    #[clap(long, default_value_t = 28.0)]
    gain_tx: f64,
    /// Gain
    #[clap(long, default_value_t = 28.0)]
    gain_rx: f64,
    /// Sample Rate
    #[clap(short, long, default_value_t = 20e6)]
    sample_rate: f64,
    /// WLAN Channel Number
    #[clap(short, long, value_parser = parse_channel, default_value = "34")]
    channel: f64,
    /// DC Offset
    #[clap(short, long, default_value_t = false)]
    dc_offset: bool,
    /// send periodic messages for testing
    #[clap(long, value_parser)]
    tx_interval: Option<f32>,
    /// TX MCS
    #[clap(long, value_parser = Mcs::parse, default_value = "qpsk12")]
    mcs: Mcs,
}

use wlan::MAX_SYM;
const PAD_FRONT: usize = 5000;
const PAD_TAIL: usize = 5000;

fn main() -> Result<()> {
    let args = Args::parse();
    println!("Configuration: {args:?}");

    let rt = Runtime::new();
    let mut fg = Flowgraph::new();

    let filter = args.device_filter.unwrap_or_else(String::new);
    let seify_dev = Device::from_args(filter).unwrap();
    let mut seify = SourceBuilder::new()
        .device(seify_dev.clone())
        .frequency(args.channel)
        .sample_rate(args.sample_rate)
        .gain(args.gain_rx);

    let src = seify.build()?;
    connect!(fg, src);

    let prev = if args.dc_offset {
        let mut avg_real = 0.0;
        let mut avg_img = 0.0;
        let ratio = 1.0e-5;
        let dc = Apply::new(move |c: &Complex32| -> Complex32 {
            avg_real = ratio * (c.re - avg_real) + avg_real;
            avg_img = ratio * (c.im - avg_img) + avg_img;
            Complex32::new(c.re - avg_real, c.im - avg_img)
        });

        connect!(fg, src > dc);
        dc
    } else {
        src
    };

    let delay = Delay::<Complex32>::new(16);
    connect!(fg, prev > delay);

    let complex_to_mag_2 = Apply::new(|i: &Complex32| i.norm_sqr());
    let float_avg = MovingAverage::<f32>::new(64);
    connect!(fg, prev > complex_to_mag_2 > float_avg);

    let mult_conj = Combine::new(|a: &Complex32, b: &Complex32| a * b.conj());
    let complex_avg = MovingAverage::<Complex32>::new(48);
    connect!(fg, prev > in0.mult_conj.out > complex_avg;
                 delay > mult_conj.in1);

    let divide_mag = Combine::new(|a: &Complex32, b: &f32| a.norm() / b);
    connect!(fg, complex_avg > divide_mag.in0; float_avg > divide_mag.in1);

    let sync_short = SyncShort::new();
    connect!(fg, delay > sync_short.in_sig;
                 complex_avg > sync_short.in_abs;
                 divide_mag > sync_short.in_cor);

    let sync_long = SyncLong::new();
    connect!(fg, sync_short > sync_long);

    let mut fft = Fft::new(64);
    fft.set_tag_propagation(Box::new(fft_tag_propagation));
    let frame_equalizer = FrameEqualizer::new();
    let decoder = Decoder::new();
    let symbol_sink = WebsocketPmtSink::new(9002);
    connect!(fg, sync_long > fft > frame_equalizer > decoder;
        frame_equalizer.symbols | symbol_sink.in);

    let (tx_frame, mut rx_frame) = mpsc::channel::<Pmt>(100);
    let message_pipe = MessagePipe::new(tx_frame);
    let udp1 = futuresdr::blocks::BlobToUdp::new("127.0.0.1:55555");
    let udp2 = futuresdr::blocks::BlobToUdp::new("127.0.0.1:55556");
    connect!(fg, decoder.rx_frames | message_pipe;
                 decoder.rx_frames | udp1;
                 decoder.rftap | udp2);

    let mut size = 4096;
    let prefix_in_size = loop {
        if size / 8 >= MAX_SYM * 64 {
            break size;
        }
        size += 4096
    };
    let mut size = 4096;
    let prefix_out_size = loop {
        if size / 8 >= PAD_FRONT + std::cmp::max(PAD_TAIL, 1) + 320 + MAX_SYM * 80 {
            break size;
        }
        size += 4096
    };

    let mac = Mac::new([0x42; 6], [0x23; 6], [0xff; 6]);
    let tx_port = mac
        .message_input_name_to_id("tx")
        .expect("No tx port found!");
    let mac = fg.add_block(mac);
    let encoder = fg.add_block(Encoder::new(args.mcs));
    fg.connect_message(mac, "tx", encoder, "tx")?;
    let mapper = fg.add_block(Mapper::new());
    fg.connect_stream(encoder, "out", mapper, "in")?;
    let mut fft = Fft::with_options(
        64,
        FftDirection::Inverse,
        true,
        Some((1.0f32 / 52.0).sqrt()),
    );
    fft.set_tag_propagation(Box::new(fft_tag_propagation));
    let fft = fg.add_block(fft);
    fg.connect_stream(mapper, "out", fft, "in")?;
    let prefix = fg.add_block(Prefix::new(PAD_FRONT, PAD_TAIL));
    fg.connect_stream_with_type(
        fft,
        "out",
        prefix,
        "in",
        Circular::with_size(prefix_in_size),
    )?;

    let mut snk = SinkBuilder::new()
        .device(seify_dev)
        .frequency(args.channel)
        .sample_rate(args.sample_rate)
        .gain(args.gain_tx);
    let snk = fg.add_block(snk.build()?);
    fg.connect_stream_with_type(
        prefix,
        "out",
        snk,
        "in",
        Circular::with_size(prefix_out_size),
    )?;

    let rt = Runtime::new();
    let (_fg, mut handle) = rt.start_sync(fg);

    rt.spawn_background(async move {
        while let Some(x) = rx_frame.next().await {
            match x {
                Pmt::Blob(data) => {
                    println!("received frame ({:?} bytes)", data.len());
                }
                _ => break,
            }
        }
    });

    let mut seq = 0u64;
    rt.block_on(async move {
        loop {
            Timer::after(Duration::from_secs_f32(args.tx_interval.unwrap_or(0.5))).await;
            println!("sending frame");
            // handle
            //     .call(
            //         mac,
            //         tx_port,
            //         Pmt::Any(Box::new((
            //             // format!("FutureSDR {seq}asdfasdfasdfasdfasdfasdfasdfasdfafasdfasdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdfasdfasdfasdfasdfasdffasdfasdfasdfasdfasdffasdfasdfasdfa").as_bytes().to_vec(),
            //             format!("FutureSDR {seq}asfasdfasdfaasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdsdfasdfasdfasdfasdfasdfasdfasdfasdfasdffasdfasdfasdfafasdfasdfasdfasdfasdfasdffasdfasdfasdfasdfasdffasdfasdfasdfa").as_bytes().to_vec(),
            //             args.mcs
            //         ))),
            //     )
            //     .await
            //     .unwrap();
            // seq += 1;

            // ~-104dBm decoding threshold for synched receiver
            // ~-100dBm for unsynchronised
            // B200mini can send with ~-50dBm @1m (gain 80)
        }
    });

    Ok(())
}

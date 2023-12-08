use clap::Parser;
use futuresdr::anyhow::Result;
use futuresdr::blocks::seify::SourceBuilder;
use futuresdr::blocks::BlobToUdp;
use futuresdr::blocks::NullSink;
use futuresdr::log::info;
use futuresdr::runtime::buffer::circular::Circular;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Runtime;
use lora::{
    optfir, Decoder, Deinterleaver, FftDemod, FrameSync, GrayMapping, HammingDec, HeaderDecoder,
    HeaderMode, MmseResampler, PfbChannelizer, StreamDeinterleaver, StreamDuplicator,
};
use rustfft::num_complex::Complex32;
use seify::Device;
use seify::Direction::Rx;

#[derive(Parser, Debug)]
#[clap(version)]
struct Args {
    /// RX Antenna
    #[clap(long)]
    rx_antenna: Option<String>,
    /// Soapy device Filter
    #[clap(long)]
    device_filter: Option<String>,
    /// Zigbee RX Gain
    #[clap(long, default_value_t = 50.0)]
    rx_gain: f64,
    /// Zigbee TX/RX Center Frequency
    #[clap(long, default_value_t = 2.45e9)]
    center_freq: f64,
    /// Zigbee RX Frequency Offset
    #[clap(long, default_value_t = 0.0)]
    rx_freq_offset: f64,
    /// Soapy RX Channel
    #[clap(long, default_value_t = 0)]
    soapy_rx_channel: usize,
    /// lora spreading factor
    #[clap(long, default_value_t = 7)]
    spreading_factor: usize,
    /// lora bandwidth
    #[clap(long, default_value_t = 125000)]
    bandwidth: usize,
    /// lora bandwidth
    #[clap(long, default_value_t = 200000)]
    channel_spacing: usize,
    /// lora bandwidth
    #[clap(long, default_value_t = 8)]
    num_channels: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let num_channels = (args.num_channels / 2) * 2 + 1; // require uneven number
    let sample_rate = (num_channels * args.channel_spacing) as f64;

    let rt = Runtime::new();
    let mut fg = Flowgraph::new();

    let filter = args.device_filter.unwrap_or_else(String::new);
    let is_soapy_dev = filter.clone().contains("driver=soapy");
    println!("is_soapy_dev: {}", is_soapy_dev);
    let seify_dev = Device::from_args(&*filter).unwrap();

    seify_dev
        // .set_sample_rate(Rx, args.soapy_rx_channel, args.sample_rate)
        .set_sample_rate(Rx, args.soapy_rx_channel, sample_rate)
        .unwrap();

    if is_soapy_dev {
        info!("setting soapy frequencies");
        // else use specified center frequency and offset
        seify_dev
            .set_component_frequency(Rx, args.soapy_rx_channel, "RF", args.center_freq)
            .unwrap();
        seify_dev
            .set_component_frequency(Rx, args.soapy_rx_channel, "BB", args.rx_freq_offset)
            .unwrap();
    } else {
        // is aaronia device, no offset for TX and only one real center freq, tx center freq has to be set as center_freq+offset; also other component names
        info!("setting aaronia frequencies");
        seify_dev
            .set_component_frequency(Rx, args.soapy_rx_channel, "RF", args.center_freq)
            .unwrap();
        seify_dev
            .set_component_frequency(Rx, args.soapy_rx_channel, "DEMOD", args.rx_freq_offset)
            .unwrap();
    }

    let mut src = SourceBuilder::new()
        .driver(if is_soapy_dev {
            "soapy"
        } else {
            "aaronia_http"
        })
        .device(seify_dev)
        .gain(args.rx_gain);
    // .dev_channels(vec![args.soapy_rx_channel]);

    if let Some(a) = args.rx_antenna {
        src = src.antenna(a);
    }

    let src = fg.add_block(src.build().unwrap());

    let soft_decoding: bool = false;

    let deinterleaver = fg.add_block(StreamDeinterleaver::<Complex32>::new(num_channels));
    fg.connect_stream(src, "out", deinterleaver, "in")?;
    // let filter_coefs = lowpass(0.208333333, &hamming(24, false));
    let transition_bw =
        (args.channel_spacing - args.bandwidth) as f64 / args.channel_spacing as f64;
    println!("{transition_bw}");
    println!("{num_channels}");
    // let transition_bw = 0.2;
    let filter_coefs = optfir::low_pass(
        1.,
        num_channels,
        0.5 - transition_bw / 2.,
        0.5 + transition_bw / 2.,
        0.1,
        100.,
        None,
    );
    let filter_coefs: Vec<f32> = filter_coefs.iter().map(|&x| x as f32).collect();
    println!("filter taps: {:?}", filter_coefs);
    let channelizer = fg.add_block(PfbChannelizer::new(num_channels, &filter_coefs, 1.));
    for i in 0..num_channels {
        fg.connect_stream(
            deinterleaver,
            format!("out{i}"),
            channelizer,
            format!("in{i}"),
        )?;
    }
    for i in 0..num_channels {
        let offset: Option<isize> = if i <= args.num_channels / 2 {
            if i < args.num_channels / 2 || args.num_channels == num_channels {
                Some(i as isize)
            } else {
                None
            }
        } else {
            Some(i as isize - num_channels as isize)
        };
        if offset.is_none() {
            let null_sink_extra_channel = fg.add_block(NullSink::<Complex32>::new());
            // map highest channel to null-sink (channel numbering starts at center and wraps around)
            fg.connect_stream(
                channelizer,
                format!("out{i}"),
                null_sink_extra_channel,
                "in",
            )?;
            println!("connecting channel {i} to NullSink");
            continue;
        }
        let resampler = fg.add_block(MmseResampler::<Complex32>::new(
            0.,
            args.channel_spacing as f32 / args.bandwidth as f32,
        ));
        fg.connect_stream(channelizer, format!("out{i}"), resampler, "in")?;
        let duplicator = fg.add_block(StreamDuplicator::<Complex32>::new(6));
        fg.connect_stream(resampler, format!("out"), duplicator, "in")?;
        let offset = offset.unwrap();
        let center_freq = args.center_freq
            + args.rx_freq_offset
            + (offset * args.channel_spacing as isize) as f64;
        println!(
            "connecting {:.1}MHz chain to channel {}",
            center_freq / 1.0e6,
            i
        );
        for sf in 7..13 {
            println!(
                "connecting {:.1}MHz FrameSync with spreading factor {sf}",
                center_freq / 1.0e6,
            );
            let frame_sync = fg.add_block(FrameSync::new(
                center_freq as u32,
                args.bandwidth as u32,
                sf,
                false,
                vec![0x12], // TODO 18?
                1,
                None,
            ));
            // fg.connect_stream_with_type(
            //     src,
            //     "out",
            //     frame_sync,
            //     "in",
            //     Circular::with_size(2 * 4 * 8192 * 4),
            // )?;
            fg.connect_stream_with_type(
                duplicator,
                format!("out{}", sf - 7),
                frame_sync,
                "in",
                Circular::with_size(2 * 4 * 8192 * 4),
            )?;
            let null_sink2 = fg.add_block(NullSink::<f32>::new());
            fg.connect_stream(frame_sync, "log_out", null_sink2, "in")?;
            let fft_demod = fg.add_block(FftDemod::new(soft_decoding, true, sf));
            fg.connect_stream(frame_sync, "out", fft_demod, "in")?;
            let gray_mapping = fg.add_block(GrayMapping::new(soft_decoding));
            fg.connect_stream(fft_demod, "out", gray_mapping, "in")?;
            let deinterleaver = fg.add_block(Deinterleaver::new(soft_decoding));
            fg.connect_stream(gray_mapping, "out", deinterleaver, "in")?;
            let hamming_dec = fg.add_block(HammingDec::new(soft_decoding));
            fg.connect_stream(deinterleaver, "out", hamming_dec, "in")?;
            let header_decoder = fg.add_block(HeaderDecoder::new(HeaderMode::Explicit, false));
            fg.connect_stream(hamming_dec, "out", header_decoder, "in")?;
            let decoder = fg.add_block(Decoder::new());
            let udp_data = fg.add_block(BlobToUdp::new("127.0.0.1:55555"));
            let udp_rftap = fg.add_block(BlobToUdp::new("127.0.0.1:55556"));
            fg.connect_message(header_decoder, "out", decoder, "in")?;
            fg.connect_message(decoder, "data", udp_data, "in")?;
            fg.connect_message(decoder, "rftap", udp_rftap, "in")?;

            fg.connect_message(header_decoder, "frame_info", frame_sync, "frame_info")?;
        }
    }

    let _ = rt.run(fg);

    Ok(())
}

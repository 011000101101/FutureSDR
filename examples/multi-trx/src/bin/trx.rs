use clap::Parser;
use std::time::Duration;

use futuresdr::futures::channel::mpsc;
use futuresdr::futures::StreamExt;

use futuresdr::anyhow::Result;
use futuresdr::async_io;
use futuresdr::async_io::block_on;
use futuresdr::async_io::Timer;
use futuresdr::blocks::seify::{SinkBuilder, SourceBuilder};
use futuresdr::blocks::Apply;
use futuresdr::blocks::Combine;
use futuresdr::blocks::Fft;
use futuresdr::blocks::FftDirection;
use futuresdr::blocks::MessagePipe;
use futuresdr::blocks::NullSink;
use futuresdr::blocks::Selector;
use futuresdr::blocks::SelectorDropPolicy as DropPolicy;
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::buffer::circular::Circular;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::Runtime;
use seify::Device;

use multitrx::MessageSelector;

use futuresdr::blocks::Delay as WlanDelay;
use wlan::fft_tag_propagation as wlan_fft_tag_propagation;
use wlan::parse_channel as wlan_parse_channel;
use wlan::Decoder as WlanDecoder;
use wlan::Encoder as WlanEncoder;
use wlan::FrameEqualizer as WlanFrameEqualizer;
use wlan::Mac as WlanMac;
use wlan::Mapper as WlanMapper;
use wlan::Mcs as WlanMcs;
use wlan::MovingAverage as WlanMovingAverage;
use wlan::Prefix as WlanPrefix;
use wlan::SyncLong as WlanSyncLong;
use wlan::SyncShort as WlanSyncShort;

use zigbee::modulator as zigbee_modulator;
use zigbee::parse_channel as zigbee_parse_channel;
use zigbee::ClockRecoveryMm as ZigbeeClockRecoveryMm;
use zigbee::Decoder as ZigbeeDecoder;
use zigbee::IqDelay as ZigbeeIqDelay;
use zigbee::Mac as ZigbeeMac;

#[derive(Parser, Debug)]
#[clap(version)]
struct Args {
    /// Soapy Filter
    #[clap(short, long)]
    filter: Option<String>,
    /// Zigbee RX Gain
    #[clap(long, default_value_t = 50.0)]
    zigbee_rx_gain: f64,
    /// Zigbee TX Gain
    #[clap(long, default_value_t = 50.0)]
    zigbee_tx_gain: f64,
    /// Zigbee RX Channel
    #[clap(id = "zigbee-rx-channel", long, value_parser = zigbee_parse_channel, default_value = "26")]
    zigbee_rx_freq: f64,
    /// Zigbee TX Channel
    #[clap(id = "zigbee-tx-channel", long, value_parser = zigbee_parse_channel, default_value = "26")]
    zigbee_tx_freq: f64,
    /// Zigbee Sample Rate
    #[clap(long, default_value_t = 4e6)]
    zigbee_sample_rate: f64,

    /// WLAN RX Channel
    #[clap(long, value_parser = wlan_parse_channel, default_value = "34")]
    wlan_rx_channel: f64,
    /// WLAN TX Channel
    #[clap(long, value_parser = wlan_parse_channel, default_value = "34")]
    wlan_tx_channel: f64,
    /// WLAN Sample Rate
    #[clap(long, default_value_t = 20e6)]
    wlan_sample_rate: f64,
    /// WLAN RX Gain
    #[clap(long, default_value_t = 60.0)]
    wlan_rx_gain: f64,
    /// WLAN TX Gain
    #[clap(long, default_value_t = 60.0)]
    wlan_tx_gain: f64,

    // Drop policy to apply on the selector.
    #[clap(short, long, default_value = "none")]
    drop_policy: DropPolicy,
}

use wlan::MAX_SYM;
const PAD_FRONT: usize = 50000;
const PAD_TAIL: usize = 50000;

fn main() -> Result<()> {
    let args = Args::parse();
    println!("Configuration: {:?}", args);

    let rx_freq = [args.wlan_rx_channel, args.zigbee_rx_freq];
    let tx_freq = [args.wlan_tx_channel, args.zigbee_tx_freq];
    let rx_gain = [args.wlan_rx_gain, args.zigbee_rx_gain];
    let tx_gain = [args.wlan_tx_gain, args.zigbee_tx_gain];
    let sample_rate = [args.wlan_sample_rate, args.zigbee_sample_rate];

    let mut fg = Flowgraph::new();

    let filter = args.filter.unwrap_or_else(|| "".to_string());
    let seify_dev = Device::from_args(&*filter).unwrap();
    let mut sink = SinkBuilder::new()
        .device(seify_dev.clone())
        .frequency(tx_freq[0])
        .sample_rate(sample_rate[0])
        .gain(tx_gain[0]);

    let mut src = SourceBuilder::new()
        .device(seify_dev)
        .frequency(rx_freq[0])
        .sample_rate(sample_rate[0])
        .gain(rx_gain[0]);

    let sink = sink.build()?;
    let src = src.build()?;

    //message handler to change frequency and sample rate during runtime
    let sink_freq_input_port_id = sink
        .message_input_name_to_id("freq")
        .expect("No freq port found!");
    let sink_sample_rate_input_port_id = sink
        .message_input_name_to_id("sample_rate")
        .expect("No sample_rate port found!");
    let sink_gain_input_port_id = sink
        .message_input_name_to_id("gain")
        .expect("No gain port found!");
    let sink = fg.add_block(sink);

    let src_freq_input_port_id = src
        .message_input_name_to_id("freq")
        .expect("No freq port found!");
    let src_sample_rate_input_port_id = src
        .message_input_name_to_id("sample_rate")
        .expect("No sample_rate port found!");
    let src_gain_input_port_id = src
        .message_input_name_to_id("gain")
        .expect("No gain port found!");
    let src = fg.add_block(src);

    //Soapy Sink + Selector
    let sink_selector = Selector::<Complex32, 2, 1>::new(args.drop_policy);
    let input_index_port_id = sink_selector
        .message_input_name_to_id("input_index")
        .expect("No input_index port found!");
    let sink_selector = fg.add_block(sink_selector);
    fg.connect_stream(sink_selector, "out0", sink, "in")?;

    //source selector
    let src_selector = Selector::<Complex32, 1, 2>::new(args.drop_policy);
    let output_index_port_id = src_selector
        .message_input_name_to_id("output_index")
        .expect("No output_index port found!");
    let src_selector = fg.add_block(src_selector);
    fg.connect_stream(src, "out", src_selector, "in0")?;

    // ========================================
    // WLAN RECEIVER
    // ========================================
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

    let (wlan_tx_frame, mut wlan_rx_frame) = mpsc::channel::<Pmt>(100);
    let wlan_message_pipe = fg.add_block(MessagePipe::new(wlan_tx_frame));
    fg.connect_message(wlan_decoder, "rx_frames", wlan_message_pipe, "in")?;
    let wlan_blob_to_udp = fg.add_block(futuresdr::blocks::BlobToUdp::new("127.0.0.1:55555"));
    fg.connect_message(wlan_decoder, "rx_frames", wlan_blob_to_udp, "in")?;
    let wlan_blob_to_udp = fg.add_block(futuresdr::blocks::BlobToUdp::new("127.0.0.1:55556"));
    fg.connect_message(wlan_decoder, "rftap", wlan_blob_to_udp, "in")?;

    // ========================================
    // ZIGBEE RECEIVER
    // ========================================
    let mut last: Complex32 = Complex32::new(0.0, 0.0);
    let mut iir: f32 = 0.0;
    let alpha = 0.00016;
    let avg = fg.add_block(Apply::new(move |i: &Complex32| -> f32 {
        let phase = (last.conj() * i).arg();
        last = *i;
        iir = (1.0 - alpha) * iir + alpha * phase;
        phase - iir
    }));

    let omega = 2.0;
    let gain_omega = 0.000225;
    let mu = 0.5;
    let gain_mu = 0.03;
    let omega_relative_limit = 0.0002;
    let mm = fg.add_block(ZigbeeClockRecoveryMm::new(
        omega,
        gain_omega,
        mu,
        gain_mu,
        omega_relative_limit,
    ));

    let zigbee_decoder = fg.add_block(ZigbeeDecoder::new(6));
    let zigbee_mac = fg.add_block(ZigbeeMac::new());
    let zigbee_snk = fg.add_block(NullSink::<u8>::new());
    let zigbee_blob_to_udp = fg.add_block(futuresdr::blocks::BlobToUdp::new("127.0.0.1:55557"));

    fg.connect_stream(src_selector, "out1", avg, "in")?;
    fg.connect_stream(avg, "out", mm, "in")?;
    fg.connect_stream(mm, "out", zigbee_decoder, "in")?;
    fg.connect_message(zigbee_decoder, "out", zigbee_mac, "rx")?;
    fg.connect_stream(zigbee_mac, "out", zigbee_snk, "in")?;
    fg.connect_message(zigbee_decoder, "out", zigbee_blob_to_udp, "in")?;

    // ========================================
    // WLAN TRANSMITTER
    // ========================================

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

    let wlan_mac = fg.add_block(WlanMac::new([0x42; 6], [0x23; 6], [0xff; 6]));
    let wlan_encoder = fg.add_block(WlanEncoder::new(WlanMcs::Qpsk_1_2));
    fg.connect_message(wlan_mac, "tx", wlan_encoder, "tx")?;
    let wlan_mapper = fg.add_block(WlanMapper::new());
    fg.connect_stream(wlan_encoder, "out", wlan_mapper, "in")?;
    let mut wlan_fft = Fft::with_options(
        64,
        FftDirection::Inverse,
        true,
        //Some((1.0f32 / 52.0).sqrt() * 0.6),
        Some((1.0f32 / 52.0).sqrt() * 0.6),
    );
    wlan_fft.set_tag_propagation(Box::new(wlan_fft_tag_propagation));
    let wlan_fft = fg.add_block(wlan_fft);
    fg.connect_stream(wlan_mapper, "out", wlan_fft, "in")?;
    let prefix = fg.add_block(WlanPrefix::new(PAD_FRONT, PAD_TAIL));
    fg.connect_stream_with_type(
        wlan_fft,
        "out",
        prefix,
        "in",
        Circular::with_size(prefix_in_size),
    )?;

    fg.connect_stream_with_type(
        prefix,
        "out",
        sink_selector,
        "in0",
        Circular::with_size(prefix_out_size),
    )?;

    // ========================================
    // ZIGBEE TRANSMITTER
    // ========================================

    let zigbee_mac = fg.add_block(ZigbeeMac::new());
    let zigbee_modulator = fg.add_block(zigbee_modulator());
    let zigbee_iq_delay = fg.add_block(ZigbeeIqDelay::new());

    fg.connect_stream(zigbee_mac, "out", zigbee_modulator, "in")?;
    fg.connect_stream(zigbee_modulator, "out", zigbee_iq_delay, "in")?;
    fg.connect_stream(zigbee_iq_delay, "out", sink_selector, "in1")?;

    // message input selector
    let message_selector = MessageSelector::new();
    let message_in_port_id = message_selector
        .message_input_name_to_id("message_in")
        .expect("No message_in port found!");
    let output_selector_port_id = message_selector
        .message_input_name_to_id("output_selector")
        .expect("No output_selector port found!");
    let message_selector = fg.add_block(message_selector);
    fg.connect_message(message_selector, "out0", wlan_mac, "tx")?;
    fg.connect_message(message_selector, "out1", zigbee_mac, "tx")?;

    let rt = Runtime::new();
    let (_fg, mut handle) = block_on(rt.start(fg));

    rt.spawn_background(async move {
        while let Some(x) = wlan_rx_frame.next().await {
            match x {
                Pmt::Blob(data) => {
                    println!("received frame ({:?} bytes)", data.len());
                }
                _ => break,
            }
        }
    });

    let mut seq = 0u64;
    let mut input_handle = handle.clone();

    rt.spawn_background(async move {
        loop {
            Timer::after(Duration::from_secs_f32(0.8)).await;
            handle
                .call(
                    message_selector,
                    message_in_port_id,
                    Pmt::Blob(format!("FutureSDR {}", seq).as_bytes().to_vec()),
                )
                .await
                .unwrap();
            seq += 1;
        }
    });

    // Keep asking user for the selection
    loop {
        println!("Enter a new output index");
        // Get input from stdin and remove all whitespace (most importantly '\n' at the end)
        let mut input = String::new(); // Input buffer
        std::io::stdin()
            .read_line(&mut input)
            .expect("error: unable to read user input");
        input.retain(|c| !c.is_whitespace());

        // If the user entered a valid number, set the new frequency and sample rate by sending a message to the `FlowgraphHandle`
        if let Ok(new_index) = input.parse::<u32>() {
            println!("Setting source index to {}", input);

            async_io::block_on(input_handle.call(
                message_selector,
                output_selector_port_id,
                Pmt::U32(new_index),
            ))?;

            async_io::block_on(input_handle.call(
                src,
                src_freq_input_port_id,
                Pmt::F64(rx_freq[new_index as usize]),
            ))?;
            async_io::block_on(input_handle.call(
                src,
                src_sample_rate_input_port_id,
                Pmt::F64(sample_rate[new_index as usize]),
            ))?;
            async_io::block_on(input_handle.call(
                src,
                src_gain_input_port_id,
                Pmt::F64(rx_gain[new_index as usize]),
            ))?;
            async_io::block_on(input_handle.call(
                src_selector,
                output_index_port_id,
                Pmt::U32(new_index),
            ))?;
            async_io::block_on(input_handle.call(
                sink,
                sink_freq_input_port_id,
                Pmt::F64(tx_freq[new_index as usize]),
            ))?;
            async_io::block_on(input_handle.call(
                sink,
                sink_sample_rate_input_port_id,
                Pmt::F64(sample_rate[new_index as usize]),
            ))?;
            async_io::block_on(input_handle.call(
                sink,
                sink_gain_input_port_id,
                Pmt::F64(tx_gain[new_index as usize]),
            ))?;
            async_io::block_on(input_handle.call(
                sink_selector,
                input_index_port_id,
                Pmt::U32(new_index),
            ))?;
        } else {
            println!("Input not parsable: {}", input);
        }
    }
}

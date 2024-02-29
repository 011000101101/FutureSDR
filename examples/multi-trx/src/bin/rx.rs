use clap::Parser;
use futuresdr::futures::channel::mpsc;
use futuresdr::futures::StreamExt;

use futuresdr::anyhow::Result;
use futuresdr::async_io;
use futuresdr::blocks::seify::SourceBuilder;
use futuresdr::blocks::Apply;
use futuresdr::blocks::Combine;
use futuresdr::blocks::Fft;
use futuresdr::blocks::MessagePipe;
use futuresdr::blocks::NullSink;
use futuresdr::blocks::Selector;
use futuresdr::blocks::SelectorDropPolicy as DropPolicy;
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::Runtime;
use seify::Device;

use futuresdr::blocks::Delay as WlanDelay;
use wlan::fft_tag_propagation as wlan_fft_tag_propagation;
use wlan::parse_channel as wlan_parse_channel;
use wlan::Decoder as WlanDecoder;
use wlan::FrameEqualizer as WlanFrameEqualizer;
use wlan::MovingAverage as WlanMovingAverage;
use wlan::SyncLong as WlanSyncLong;
use wlan::SyncShort as WlanSyncShort;

use zigbee::parse_channel as zigbee_parse_channel;
use zigbee::ClockRecoveryMm as ZigbeeClockRecoveryMm;
use zigbee::Decoder as ZigbeeDecoder;
use zigbee::Mac as ZigbeeMac;

#[derive(Parser, Debug)]
#[clap(version)]
struct Args {
    /// Antenna
    #[clap(short, long)]
    antenna: Option<String>,
    /// Soapy Filter
    #[clap(short, long)]
    filter: Option<String>,
    /// Gain
    #[clap(short, long, default_value_t = 60.0)]
    gain: f64,
    /// WLAN Sample Rate
    #[clap(long, default_value_t = 20e6)]
    wlan_sample_rate: f64,
    /// WLAN Channel Number
    #[clap(long, value_parser = wlan_parse_channel, default_value = "34")]
    wlan_channel: f64,
    /// Zigbee Sample Rate
    #[clap(long, default_value_t = 4e6)]
    zigbee_sample_rate: f64,
    /// Zigbee Channel Number (11..26)
    #[clap(long, value_parser= zigbee_parse_channel, default_value = "26")]
    zigbee_channel: f64,
    // Drop policy to apply on the selector.
    #[clap(short, long, default_value = "same")]
    drop_policy: DropPolicy,
}

fn main() -> Result<()> {
    let args = Args::parse();
    println!("Configuration {:?}", args);

    let freq = [args.wlan_channel, args.zigbee_channel];

    let sample_rate = [args.wlan_sample_rate, args.zigbee_sample_rate];

    let mut fg = Flowgraph::new();

    let selector = Selector::<Complex32, 1, 2>::new(args.drop_policy);
    let output_index_port_id = selector
        .message_input_name_to_id("output_index")
        .expect("No output_index port found!");
    let selector = fg.add_block(selector);

    let filter = args.filter.unwrap_or_else(|| "".to_string());
    let seify_dev = Device::from_args(&*filter).unwrap();
    let mut soapy = SourceBuilder::new()
        .device(seify_dev)
        .frequency(freq[0])
        .sample_rate(sample_rate[0])
        .gain(args.gain);
    if let Some(a) = args.antenna {
        soapy = soapy.antenna(a);
    }
    let soapy = soapy.build()?;

    //message handler to change frequency and sample rate during runtime
    let freq_input_port_id = soapy
        .message_input_name_to_id("freq")
        .expect("No freq port found!");
    let sample_rate_input_port_id = soapy
        .message_input_name_to_id("sample_rate")
        .expect("No sample_rate port found!");
    let src = fg.add_block(soapy);

    fg.connect_stream(src, "out", selector, "in0")?;

    // ========================================
    // WLAN RECEIVER
    // ========================================
    let wlan_delay = fg.add_block(WlanDelay::<Complex32>::new(16));
    fg.connect_stream(selector, "out0", wlan_delay, "in")?;

    let wlan_complex_to_mag_2 = fg.add_block(Apply::new(|i: &Complex32| i.norm_sqr()));
    let wlan_float_avg = fg.add_block(WlanMovingAverage::<f32>::new(64));
    fg.connect_stream(selector, "out0", wlan_complex_to_mag_2, "in")?;
    fg.connect_stream(wlan_complex_to_mag_2, "out", wlan_float_avg, "in")?;

    let wlan_mult_conj = fg.add_block(Combine::new(|a: &Complex32, b: &Complex32| a * b.conj()));
    let wlan_complex_avg = fg.add_block(WlanMovingAverage::<Complex32>::new(48));
    fg.connect_stream(selector, "out0", wlan_mult_conj, "in0")?;
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

    fg.connect_stream(selector, "out1", avg, "in")?;
    fg.connect_stream(avg, "out", mm, "in")?;
    fg.connect_stream(mm, "out", zigbee_decoder, "in")?;
    fg.connect_stream(zigbee_mac, "out", zigbee_snk, "in")?;
    fg.connect_message(zigbee_decoder, "out", zigbee_mac, "rx")?;
    fg.connect_message(zigbee_decoder, "out", zigbee_blob_to_udp, "in")?;

    // Start the flowgraph and save the handle
    let rt = Runtime::new();
    let (_res, mut handle) = async_io::block_on(rt.start(fg));

    //WLAN frame received message currently intterupts user input to select source
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
            async_io::block_on(handle.call(
                src,
                freq_input_port_id,
                Pmt::F64(freq[new_index as usize]),
            ))?;
            async_io::block_on(handle.call(
                src,
                sample_rate_input_port_id,
                Pmt::F64(sample_rate[new_index as usize]),
            ))?;
            async_io::block_on(handle.call(selector, output_index_port_id, Pmt::U32(new_index)))?;
        } else {
            println!("Input not parsable: {}", input);
        }
    }
}

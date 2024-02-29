use clap::Parser;
use std::sync::mpsc::channel;
use std::time::Duration;

use futuresdr::anyhow::Result;
use futuresdr::async_io;
use futuresdr::async_io::{block_on, Timer};
use futuresdr::blocks::seify::SinkBuilder;
use futuresdr::blocks::Fft;
use futuresdr::blocks::FftDirection;
use futuresdr::blocks::Selector;
use futuresdr::blocks::SelectorDropPolicy as DropPolicy;
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::buffer::circular::Circular;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::Runtime;
use seify::Device;

use wlan::fft_tag_propagation as wlan_fft_tag_propagation;
use wlan::parse_channel as wlan_parse_channel;
use wlan::Encoder as WlanEncoder;
use wlan::Mac as WlanMac;
use wlan::Mapper as WlanMapper;
use wlan::Mcs as WlanMcs;
use wlan::Prefix as WlanPrefix;

use zigbee::modulator as zigbee_modulator;
use zigbee::parse_channel as zigbee_parse_channel;
use zigbee::IqDelay as ZigbeeIqDelay;
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
    #[clap(short, long, default_value = "none")]
    drop_policy: DropPolicy,
}

use wlan::MAX_SYM;
const PAD_FRONT: usize = 5000;
const PAD_TAIL: usize = 5000;

fn main() -> Result<()> {
    let args = Args::parse();
    println!("Configuration {:?}", args);

    let freq = [args.wlan_channel, args.zigbee_channel];

    let sample_rate = [args.wlan_sample_rate, args.zigbee_sample_rate];

    let mut fg = Flowgraph::new();

    let selector = Selector::<Complex32, 2, 1>::new(args.drop_policy);
    let input_index_port_id = selector
        .message_input_name_to_id("input_index")
        .expect("No input_index port found!");
    let selector = fg.add_block(selector);

    let filter = args.filter.unwrap_or_else(|| "".to_string());
    let seify_dev = Device::from_args(&*filter).unwrap();
    let mut soapy = SinkBuilder::new()
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
    let sink = fg.add_block(soapy);

    fg.connect_stream(selector, "out0", sink, "in")?;

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
        selector,
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
    fg.connect_stream(zigbee_iq_delay, "out", selector, "in1")?;

    // Start the flowgraph and save the handle
    let rt = Runtime::new();
    let (_fg, mut handle) = block_on(rt.start(fg));

    //mode 0 = WLAN, Zigbee = 1
    let mut seq = 0u64;

    let mut input_handle = handle.clone();

    let (sender, receiver) = channel();

    let mut mode = 0;

    rt.spawn_background(async move {
        loop {
            Timer::after(Duration::from_secs_f32(0.8)).await;
            if let Some(new_mode) = receiver.try_recv().ok() {
                mode = new_mode;
            }
            println!("Mode {:?}", mode);
            //WLAN message
            if mode == 0 {
                handle
                    .call(
                        wlan_mac,
                        0,
                        Pmt::Any(Box::new((
                            format!("FutureSDR {}", seq).as_bytes().to_vec(),
                            WlanMcs::Qpsk_1_2,
                        ))),
                    )
                    .await
                    .unwrap();
            }
            //Zigbee message
            if mode == 1 {
                handle
                    .call(
                        zigbee_mac,
                        1,
                        Pmt::Blob(format!("FutureSDR {}", seq).as_bytes().to_vec()),
                    )
                    .await
                    .unwrap();
            }
            seq += 1;
        }
    });

    // Keep asking user for the selection
    loop {
        println!("Enter a new Input index");
        // Get input from stdin and remove all whitespace (most importantly '\n' at the end)
        let mut input = String::new(); // Input buffer
        std::io::stdin()
            .read_line(&mut input)
            .expect("error: unable to read user input");
        input.retain(|c| !c.is_whitespace());

        // If the user entered a valid number, set the new frequency and sample rate by sending a message to the `FlowgraphHandle`
        if let Ok(new_index) = input.parse::<u32>() {
            sender.send(new_index)?;
            println!("Setting source index to {}", input);

            async_io::block_on(input_handle.call(
                sink,
                freq_input_port_id,
                Pmt::F64(freq[new_index as usize]),
            ))?;
            println!("Set frequency to {:?}", freq[new_index as usize]);

            async_io::block_on(input_handle.call(
                sink,
                sample_rate_input_port_id,
                Pmt::F64(sample_rate[new_index as usize]),
            ))?;
            println!("Set  sample rate to {:?}", sample_rate[new_index as usize]);

            async_io::block_on(input_handle.call(
                selector,
                input_index_port_id,
                Pmt::U32(new_index),
            ))?;
            println!("Set selector input to {:?}", new_index);
        } else {
            println!("Input not parsable: {}", input);
        }
    }
}

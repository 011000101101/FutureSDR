use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use futuresdr::async_io::Timer;
use futuresdr::blocks::seify::SinkBuilder;
use futuresdr::blocks::BurstPad;
use futuresdr::blocks::BurstSizeRewriter;
use futuresdr::macros::connect;
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::BlockT;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::Runtime;
use futuresdr::tracing::info;
use lora::default_values::ldro;
use lora::utils::Bandwidth;
use lora::utils::Channel;
use lora::utils::CodeRate;
use lora::utils::SpreadingFactor;
use lora::Transmitter;
use rand::RngCore;

#[derive(Parser, Debug)]
struct Args {
    /// TX Antenna
    #[clap(long)]
    antenna: Option<String>,
    /// Seify Device Args
    #[clap(short, long)]
    args: Option<String>,
    /// TX Gain
    #[clap(short, long, default_value_t = 50.0)]
    gain: f64,
    /// Oversampling Factor
    #[clap(short, long, default_value_t = 4)]
    oversampling: usize,
    /// Center Frequency
    #[clap(long, value_enum, default_value_t = Channel::EU868_1)]
    channel: Channel,
    /// Send periodic messages for testing
    #[clap(short, long, default_value_t = 2.0)]
    tx_interval: f32,
    /// Spreading Factor
    #[clap(short, long, value_enum, default_value_t = SpreadingFactor::SF7)]
    spreading_factor: SpreadingFactor,
    /// Sync Word
    #[clap(long, default_value_t = 0x0816)]
    sync_word: usize,
    /// Sync Word
    #[clap(long, default_value_t = 16)]
    payload_len: usize,
    /// LoRa Bandwidth
    #[clap(short, long, value_enum, default_value_t = Bandwidth::BW125)]
    bandwidth: Bandwidth,
    /// LoRa Code Rate
    #[clap(short, long, value_enum, default_value_t = CodeRate::CR_4_5)]
    code_rate: CodeRate,
}

const HAS_CRC: bool = true;
const IMPLICIT_HEADER: bool = false;
const LOW_DATA_RATE: bool = false;
const PREAMBLE_LEN: usize = 8;
const PAD: usize = 10000;

fn main() -> Result<()> {
    let args = Args::parse();

    let mut fg = Flowgraph::new();

    let sink = SinkBuilder::new()
        .sample_rate((Into::<usize>::into(args.bandwidth) * args.oversampling) as f64)
        .frequency(args.channel.into())
        .gain(args.gain)
        .antenna(args.antenna)
        .args(args.args)?
        .build()?;

    let transmitter = Transmitter::new(
        args.code_rate,
        HAS_CRC,
        args.spreading_factor,
        ldro(args.spreading_factor),
        IMPLICIT_HEADER,
        args.oversampling,
        vec![args.sync_word],
        PREAMBLE_LEN,
        PAD,
    );
    let fg_tx_port = transmitter
        .message_input_name_to_id("msg")
        .expect("No message_in port found!");
    let pad_head = BurstPad::new(0, 524288, Complex32::new(0.0, 0.0));
    let pad = BurstSizeRewriter::new_for_hackrf();
    let strip_tags = futuresdr::blocks::Copy::<Complex32>::new();

    connect!(fg, transmitter > pad_head > pad > strip_tags > sink);

    let rt = Runtime::new();

    let (_fg, mut handle) = rt.start_sync(fg);
    rt.block_on(async move {
        let mut payload = vec![0u8; args.payload_len];
        loop {
            rand::rng().fill_bytes(payload.as_mut());
            handle
                .call(transmitter, fg_tx_port, Pmt::Blob(payload.clone()))
                .await
                .unwrap();
            info!("sending frame with payload {:?}", payload);
            Timer::after(Duration::from_secs_f32(args.tx_interval)).await;
        }
    });

    Ok(())
}

use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use futuresdr::async_io::Timer;
use futuresdr::blocks::BurstPad;
use futuresdr::blocks::BurstSizeRewriter;
use futuresdr::blocks::BurstSplit;
use futuresdr::blocks::seify::Builder;
use futuresdr::macros::connect;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::Runtime;
use futuresdr::tracing::info;
use lora::Transmitter;
use lora::default_values::HAS_CRC;
use lora::default_values::IMPLICIT_HEADER;
use lora::default_values::PREAMBLE_LEN;
use lora::default_values::ldro;
use lora::utils::Bandwidth;
use lora::utils::Channel;
use lora::utils::ChannelEnumParser;
use lora::utils::CodeRate;
use lora::utils::SpreadingFactor;
use lora::utils::sample_count;
use rand::RngCore;

#[derive(Parser, Debug)]
struct Args {
    /// TX Antenna
    #[clap(long)]
    antenna: Option<String>,
    /// TX Gain
    #[clap(short, long, default_value_t = 50.0)]
    gain: f64,
    /// Oversampling Factor
    #[clap(short, long, default_value_t = 4)]
    oversampling: usize,
    /// Center Frequency
    #[clap(long, value_enum, value_parser=ChannelEnumParser, default_value_t = Channel::EU868_1)]
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
const PAD: usize = 0;
const ARGS: &str = "driver=soapy,soapy_driver=hackrf";

fn main() -> Result<()> {
    let args = Args::parse();

    let mut fg = Flowgraph::new();

    let transmitter: Transmitter = Transmitter::new(
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
    let mut max_burst_size = sample_count(
        // make sure the sink will not stall on large bursts
        args.spreading_factor,
        PREAMBLE_LEN,
        IMPLICIT_HEADER,
        255,
        HAS_CRC,
        args.code_rate,
        args.oversampling,
        PAD,
        ldro(args.spreading_factor),
    );
    let pad_head: BurstPad = BurstPad::new_for_hackrf();
    max_burst_size = pad_head.propagate_max_burst_size(max_burst_size);
    let pad: BurstSizeRewriter = BurstSizeRewriter::new_for_hackrf();
    max_burst_size = pad.propagate_max_burst_size(max_burst_size);
    let split_bursts: BurstSplit = BurstSplit::new_for_hackrf();
    max_burst_size = split_bursts.propagate_max_burst_size(max_burst_size);

    let sink = Builder::new(ARGS)?
        .sample_rate((Into::<usize>::into(args.bandwidth) * args.oversampling) as f64)
        .frequency(args.channel.into())
        .gain(args.gain)
        .antenna(args.antenna)
        .min_in_buffer_size(max_burst_size)
        .build_sink()?;

    connect!(fg, transmitter > pad_head > pad
        > split_bursts
        > inputs[0].sink);
    let transmitter = transmitter.into();

    let rt = Runtime::new();

    let (_fg, mut handle) = rt.start_sync(fg)?;
    rt.block_on(async move {
        let mut payload = vec![0u8; args.payload_len];
        loop {
            rand::rng().fill_bytes(payload.as_mut());
            handle
                .call(transmitter, "msg", Pmt::Blob(payload.clone()))
                .await
                .unwrap();
            info!("sending frame with payload {:02x?}", payload);
            Timer::after(Duration::from_secs_f32(args.tx_interval)).await;
        }
    });

    Ok(())
}

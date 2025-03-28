use rustfft::num_complex::Complex32;

use futuredsp::firdes::remez;
use futuresdr::anyhow::Result;
use futuresdr::blocks::FileSink;
use futuresdr::blocks::FileSource;
use futuresdr::blocks::PfbChannelizer;
use futuresdr::blocks::PfbSynthesizer;
use futuresdr::macros::connect;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Runtime;

const NUM_CHANNELS: usize = 10;
const CHANNEL_SPACING: usize = 100_000;
const BANDWIDTH: usize = 62_500;

fn main() -> Result<()> {
    let rt = Runtime::new();
    let mut fg = Flowgraph::new();

    let src =
        FileSource::<Complex32>::new("/home/vmechler/Documents/lora/very-long-slow.cf32", false);

    let transition_bw = (CHANNEL_SPACING - BANDWIDTH) as f64 / CHANNEL_SPACING as f64;
    let channelizer_taps: Vec<f32> = remez::low_pass(
        1.,
        NUM_CHANNELS,
        0.5 - transition_bw / 2.,
        0.5 + transition_bw / 2.,
        0.1,
        100.,
        None,
    )
    .into_iter()
    .map(|x| x as f32)
    .collect();
    let channelizer = fg.add_block(PfbChannelizer::new(NUM_CHANNELS, &channelizer_taps, 1.0));
    let synthesizer = fg.add_block(PfbSynthesizer::new(NUM_CHANNELS, &channelizer_taps));
    connect!(fg, src > channelizer);
    for n_out in 0..NUM_CHANNELS {
        fg.connect_stream(
            channelizer,
            format!("out{n_out}"),
            synthesizer,
            format!("in{n_out}"),
        )?;
    }
    let sink = fg.add_block(FileSink::<Complex32>::new(
        "/home/vmechler/Documents/lora/synthesizer_test.cf32",
    ));
    connect!(fg, synthesizer > sink);

    let _ = rt.run(fg);

    Ok(())
}

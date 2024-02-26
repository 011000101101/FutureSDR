use chrono::prelude::*;
use clap::Parser;
use futuresdr::anyhow::Result;
use futuresdr::async_io::Timer;
use futuresdr::blocks::seify::SinkBuilder;
use futuresdr::blocks::seify::SourceBuilder;
use futuresdr::blocks::FirBuilder;
use futuresdr::blocks::{BlobToUdp, Split};
use futuresdr::blocks::{FileSink, NullSink};
use futuresdr::macros::connect;
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::buffer::circular::Circular;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Runtime;
use seify::Device;

#[derive(Parser, Debug)]
#[clap(version)]
struct Args {
    /// Soapy device Filter
    #[clap(long)]
    device_filter: Option<String>,
    /// RX Gain
    #[clap(long, default_value_t = 50.0)]
    gain: f64,
    /// Zigbee Sample Rate
    #[clap(long, default_value_t = 125e3)]
    sample_rate: f64,
    /// Zigbee TX/RX Center Frequency
    #[clap(long, default_value_t = 868.1e6)]
    center_freq: f64,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let rt = Runtime::new();
    let mut fg = Flowgraph::new();

    let filter = args.device_filter.unwrap_or_else(String::new);
    let is_soapy_dev = filter.clone().contains("driver=soapy");
    println!("is_soapy_dev: {}", is_soapy_dev);
    let seify_dev = Device::from_args(&*filter).unwrap();
    println!("success.");

    let mut src = SourceBuilder::new()
        .device(seify_dev)
        .sample_rate(args.sample_rate as f64)
        .frequency(args.center_freq)
        .gain(args.gain);

    let src = fg.add_block(src.build().unwrap());

    let filesink = FileSink::<Complex32>::new(format!(
        "/tmp/rx_capture_{}.bin",
        Local::now().format("%Y-%m-%d_%H-%M")
    ));

    connect!(
        fg,
        src
        [Circular::with_size(2 * 4 * 8192 * 4)]
        filesink
    );

    let _ = rt.run(fg);

    Ok(())
}

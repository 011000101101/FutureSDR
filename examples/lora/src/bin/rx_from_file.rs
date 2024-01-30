use clap::Parser;
use futuresdr::anyhow::Result;
use futuresdr::blocks::BlobToUdp;
use futuresdr::blocks::FileSource;
use futuresdr::blocks::FirBuilder;
use futuresdr::blocks::NullSink;
use futuresdr::macros::connect;
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::buffer::circular::Circular;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Runtime;
use seify::Device;

use lora::Decoder;
use lora::Deinterleaver;
use lora::FftDemod;
use lora::FrameSync;
use lora::GrayMapping;
use lora::HammingDec;
use lora::HeaderDecoder;
use lora::HeaderMode;

#[derive(Parser, Debug)]
#[clap(version)]
struct Args {
    /// input file path dumped with a FileSink::<Complex32> block
    #[clap(long)]
    input_file: String,
    /// RX Frequency
    #[clap(long, default_value_t = 868.1e6)]
    frequency: f64,
    /// LoRa Spreading Factor
    #[clap(long, default_value_t = 7)]
    spreading_factor: usize,
    /// LoRa Bandwidth
    #[clap(long, default_value_t = 125000)]
    bandwidth: usize,
    /// LoRa Sync Word
    #[clap(long, default_value_t = 0x12)]
    sync_word: u8,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let soft_decoding: bool = false;

    let rt = Runtime::new();
    let mut fg = Flowgraph::new();

    let mut src = FileSource::<Complex32>::new(args.input_file, false);

    // let downsample =
    //     FirBuilder::new_resampling::<Complex32, Complex32>(1, 1000000 / args.bandwidth);
    let frame_sync = FrameSync::new(
        args.frequency as u32,
        args.bandwidth as u32,
        args.spreading_factor,
        false,
        vec![args.sync_word.into()],
        1,
        None,
    );
    let null_sink = NullSink::<f32>::new();
    let fft_demod = FftDemod::new(soft_decoding, true, args.spreading_factor);
    let gray_mapping = GrayMapping::new(soft_decoding);
    let deinterleaver = Deinterleaver::new(soft_decoding);
    let hamming_dec = HammingDec::new(soft_decoding);
    let header_decoder = HeaderDecoder::new(HeaderMode::Explicit, false);
    let decoder = Decoder::new();
    let udp_data = BlobToUdp::new("127.0.0.1:55555");
    let udp_rftap = BlobToUdp::new("127.0.0.1:55556");

    connect!(fg, src
        // > downsample
        [Circular::with_size(2 * 4 * 8192 * 4)] frame_sync > fft_demod > gray_mapping > deinterleaver > hamming_dec > header_decoder;
        frame_sync.log_out > null_sink;
        header_decoder.frame_info | frame_sync.frame_info;
        header_decoder | decoder;
        decoder.data | udp_data;
        decoder.rftap | udp_rftap;
    );
    let _ = rt.run(fg);

    Ok(())
}

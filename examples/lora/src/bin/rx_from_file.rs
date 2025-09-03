use anyhow::Result;
use anyhow::anyhow;
use futuredsp::firdes;
use futuresdr::blocks::BlobToUdp;
use futuresdr::blocks::Delay;
use futuresdr::blocks::FileSource;
use futuresdr::blocks::XlatingFirBuilder;
use futuresdr::macros::connect;
use futuresdr::num_complex::Complex32;
use futuresdr::runtime::Flowgraph;
use futuresdr::runtime::Runtime;
use futuresdr::runtime::buffer::circular::Circular;
use futuresdr::tracing::error;
use lora::Decoder;
use lora::Deinterleaver;
use lora::FftDemod;
use lora::FrameSync;
use lora::GrayMapping;
use lora::HammingDec;
use lora::HeaderDecoder;
use lora::HeaderMode;
use lora::utils::SpreadingFactor;

const SOFT_DECODING: bool = false;
const IMPLICIT_HEADER: bool = false;
const LDRO_MODE: bool = true;
const BW: usize = 62500;

fn main() -> Result<()> {
    futuresdr::runtime::init();

    let src =
        FileSource::<Complex32>::new("/home/vmechler/Documents/lora/very-long-slow.cf32", false);
    let skip = Delay::<Complex32>::new(1_000_000 * -5);
    let cutoff = BW as f64 / 2.0 / 1e6;
    let transition_bw = BW as f64 / 10.0 / 1e6;
    let taps = firdes::kaiser::lowpass(cutoff, transition_bw, 0.05);
    let decimation = XlatingFirBuilder::with_taps(taps, 4, 200e3, 1e6);

    let frame_sync = FrameSync::new(
        869492500,
        BW,
        SpreadingFactor::SF12.into(),
        IMPLICIT_HEADER,
        vec![vec![16, 88]],
        4,
        None,
        Some("header_crc_ok"),
        false,
        None,
    );
    let fft_demod = FftDemod::new(SOFT_DECODING, SpreadingFactor::SF12.into(), LDRO_MODE);
    let gray_mapping = GrayMapping::new(SOFT_DECODING);
    let deinterleaver = Deinterleaver::new(SOFT_DECODING, LDRO_MODE, SpreadingFactor::SF12);
    let hamming_dec = HammingDec::new(SOFT_DECODING);
    let header_decoder = HeaderDecoder::new(HeaderMode::Explicit, LDRO_MODE);
    let decoder = Decoder::new();
    let udp_data = BlobToUdp::new("127.0.0.1:55555");
    let udp_rftap = BlobToUdp::new("127.0.0.1:55556");

    let mut fg = Flowgraph::new();
    connect!(fg,
        src > skip > decimation [Circular::with_size((1 << 12) * 64)] frame_sync [Circular::with_size((1 << 12) * 64)] fft_demod > gray_mapping > deinterleaver > hamming_dec > header_decoder;
        header_decoder.frame_info | frame_sync.frame_info;
        header_decoder | decoder;
        decoder.out | udp_data;
        decoder.rftap | udp_rftap;
    );

    // let channel = MeshtasticChannel::new("FOO", "AQ==");
    // let payload = String::from("asdfasdf");
    // let data = channel.encode(payload);
    // info!("{:?}", data);

    if let Err(e) = Runtime::new().run(fg) {
        error!("{}", &e);
        return Err(anyhow!("{}", e));
    }
    Ok(())
}

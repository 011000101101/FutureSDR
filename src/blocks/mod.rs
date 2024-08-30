//! ## Block Library
//! ## Functional/Apply-style Blocks
//! | Block | Usage | WebAssembly? |
//! |---|---|---|
//! | [Apply] | Apply a function to each sample. | ✅ |
//! | [ApplyIntoIter] | Apply a function on each input sample to create an iterator and output its values. | ✅ |
//! | [ApplyNM] | Apply a function to each N input samples, producing M output samples. | ✅ |
//! | [Combine] | Apply a function to combine two streams into one. | ✅ |
//! | [Filter] | Apply a function, returning an [Option] to allow filtering samples. | ✅ |
//! | [Sink] | Apply a function to received samples. | ✅ |
//! | [Source] | Repeatedly apply a function to generate samples. | ✅ |
//! | [Split] | Apply a function to split a stream. | ✅ |
//! | [FiniteSource] | Repeatedly apply a function to generate samples, using [Option] values to allow termination. | ✅ |
//!
//! ## Streams
//! | Block | Usage | WebAssembly? |
//! |---|---|---|
//! | [StreamDeinterleaver](StreamDeinterleaver) | Stream Deinterleave | ✅ |
//! | [StreamDuplicator](StreamDuplicator) | Stream Duplicator | ✅ |
//!
//! ## DSP blocks
//! | Block | Usage | WebAssembly? |
//! |---|---|---|
//! | [Fft](Fft) | Compute an FFT. | ✅ |
//! | [Fir](FirBuilder) | FIR filter and resampler. | ✅ |
//! | [Iir](IirBuilder) | IIR filter. | ✅ |
//! | [PfbArbResampler](PfbArbResampler) | Polyphase Arbitrary Rate Resampler | ✅ |
//! | [PfbChannelizer](PfbChannelizer) | Polyphase Channelizer | ✅ |
//! | [PfbSynthesizer](PfbSynthesizer) | Polyphase Synthesizer | ✅ |
//! | [XlatingFir](XlatingFirBuilder) | Xlating FIR filter and decimator. | ✅ |
//!
//! ## Misc
//! | Block | Usage | WebAssembly? |
//! |---|---|---|
//! | [ConsoleSink] | Log stream data with [log::info!]. | ✅ |
//! | [Delay] | Delays samples. | ✅ |
//! | [Head] | Copies only a given number of samples and stops. | ✅ |
//! | [NullSink] | Drops samples. | ✅ |
//! | [NullSource] | Generates a stream of zeros. | ✅ |
//! | [Selector] | Forward the input stream with a given index to the output stream with a given index. | ✅ |
//! | [TagDebug] | Drop samples, printing tags. | ✅ |
//! | [Throttle] | Limit sample rate. | ✅ |
//! | [VectorSink] | Store received samples in vector. | ✅ |
//! | [VectorSource] | Stream samples from vector. | ✅ |
//!
//! ## Message Passing
//! | Block | Usage | WebAssembly? |
//! |---|---|---|
//! | [MessageAnnotator] | Wrap every message in a DictStrPmt and add fixed additional fields, to facilitate multiplexing w/o losing the source association | ✅ |
//! | [MessageBurst] | Output a given number of messages in one burst and terminate. | ✅ |
//! | [MessageCopy] | Forward messages. | ✅ |
//! | [MessagePipe] | Push received messages into a channel. | ✅ |
//! | [MessageSink] | Black hole for messages. | ✅ |
//! | [MessageSource](MessageSourceBuilder) | Output the same message periodically. | ✅ |
//!
//! ## Performance Evaluation
//! | Block | Usage | WebAssembly? | Feature |
//! |---|---|---|---|
//! | [struct@Copy] | Copy input samples to the output. | ✅ | |
//! | [CopyRand] | Copy input samples to the output, forwarding only a randomly selected number of samples. | ❌ | |
//! | lttng::NullSource | Null source that calls an [lttng](https://lttng.org/) tracepoint for every batch of produced samples. | ❌ | lttng |
//! | lttng:NullSink | Null sink that calls an [lttng](https://lttng.org/) tracepoint for every batch of received samples. | ❌ | lttng |
//!
//! ## I/O
//! | Block | Usage | WebAssembly? |
//! |---|---|---|
//! | [BlobToUdp] | Push [Blobs](crate::runtime::Pmt::Blob) into a UDP socket. | ❌ |
//! | [ChannelSource] | Push samples through a channel into a stream connection. | ✅ |
//! | [ChannelSink] | Read samples from Flowgraph and send them into a channel | ✅ |
//! | [FileSink] | Write samples to a file. | ❌ |
//! | [FileSource] | Read samples from a file. | ❌ |
//! | [TcpSource] | Reads samples from a TCP socket. | ❌ |
//! | [TcpSink] | Push samples into a TCP socket. | ❌ |
//! | [UdpSource] | Reads samples from a UDP socket. | ❌ |
//! | [WebsocketSink] | Push samples in a WebSocket. | ❌ |
//! | [WebsocketPmtSink] | Push samples from Pmts a WebSocket. | ❌ |
//! | [zeromq::PubSink] | Push samples into [ZeroMQ](https://zeromq.org/) socket. | ❌ |
//! | [zeromq::SubSource] | Read samples from [ZeroMQ](https://zeromq.org/) socket. | ❌ |
//!
//! ## SDR Hardware
//! | Block | Usage | Feature | WebAssembly? |
//! |---|---|---|---|
//! | [SeifySink](seify::SinkBuilder) | Transmit samples with a Seify device. | seify | ❌ |
//! | [SeifySource](seify::SourceBuilder) | Receive samples from a Seify device. | seify | ❌ |
//!
//! ## Hardware Acceleration
//! | Block | Usage | WebAssembly? | Feature |
//! |---|---|---|---|
//! | [Vulkan] | Interface GPU w/ Vulkan. | ❌ | `vulkan` |
//! | [Wgpu] | Interface GPU w/ native API. | ✅ | `wgpu` |
//! | [Zynq] | Interface Zynq FPGA w/ AXI DMA (async mode). | ❌ | `zynq` |
//! | [ZynqSync] | Interface Zynq FPGA w/ AXI DMA (sync mode). | ❌ | `zynq` |
//!
//! ## WASM-specific (target `wasm32-unknown-unknown`)
//! | Block | Usage | WebAssembly? |
//! |---|---|---|
//! | HackRf | WASM + WebUSB source for HackRF. | ✅ |
//! | WasmWsSink | Send samples via a WebSocket. | ✅ |
//!
//! ## Signal Sources
//! | Block | Usage | WebAssembly? |
//! |---|---|---|
//! | [SignalSource](SignalSourceBuilder) | Create signals (sin, cos, square). | ✅ |
//!
//! ## Audio (requires `audio` feature)
//! | Block | Usage | WebAssembly? |
//! |---|---|---|
//! | [AudioSink](audio::AudioSink) | Audio sink. | ❌ |
//! | [AudioSource](audio::AudioSource) | Audio source. | ❌ |
//! | [FileSource](audio::FileSource) | Read an audio file and output its samples. | ❌ |
//! | [WavSink](audio::WavSink) | Writes samples to a WAV file | ❌ |
//!

pub use apply::Apply;
pub use applyintoiter::ApplyIntoIter;
pub use applynm::ApplyNM;
#[cfg(not(target_arch = "wasm32"))]
pub use blob_to_udp::BlobToUdp;
pub use channel_sink::ChannelSink;
pub use channel_source::ChannelSource;
pub use combine::Combine;
pub use console_sink::ConsoleSink;
pub use copy::Copy;
pub use copy_rand::{CopyRand, CopyRandBuilder};
pub use delay::Delay;
pub use fft::Fft;
pub use fft::FftDirection;
#[cfg(not(target_arch = "wasm32"))]
pub use file_sink::FileSink;
#[cfg(not(target_arch = "wasm32"))]
pub use file_source::FileSource;
pub use filter::Filter;
pub use finite_source::FiniteSource;
pub use fir::Fir;
pub use fir::FirBuilder;
pub use head::Head;
pub use iir::{Iir, IirBuilder};
pub use message_annotator::MessageAnnotator;
pub use message_burst::MessageBurst;
pub use message_copy::MessageCopy;
pub use message_pipe::MessagePipe;
pub use message_sink::MessageSink;
#[cfg(not(target_arch = "wasm32"))]
pub use message_source::{MessageSource, MessageSourceBuilder};
pub use null_sink::NullSink;
pub use null_source::NullSource;
pub use pfb::arb_resampler::PfbArbResampler;
pub use pfb::channelizer::PfbChannelizer;
pub use pfb::synthesizer::PfbSynthesizer;
pub use selector::DropPolicy as SelectorDropPolicy;
pub use selector::Selector;
pub use signal_source::FixedPointPhase;
pub use signal_source::SignalSourceBuilder;
pub use sink::Sink;
pub use source::Source;
pub use split::Split;
pub use stream_deinterleaver::StreamDeinterleaver;
pub use stream_duplicator::StreamDuplicator;
pub use tag_debug::TagDebug;
#[cfg(not(target_arch = "wasm32"))]
pub use tcp_sink::TcpSink;
#[cfg(not(target_arch = "wasm32"))]
pub use tcp_source::TcpSource;
pub use throttle::Throttle;
#[cfg(not(target_arch = "wasm32"))]
pub use udp_source::UdpSource;
pub use vector_sink::{VectorSink, VectorSinkBuilder};
pub use vector_source::VectorSource;
#[cfg(feature = "vulkan")]
pub use vulkan::{Vulkan, VulkanBuilder};
#[cfg(not(target_arch = "wasm32"))]
pub use websocket_pmt_sink::WebsocketPmtSink;
#[cfg(not(target_arch = "wasm32"))]
pub use websocket_sink::{WebsocketSink, WebsocketSinkBuilder, WebsocketSinkMode};
pub use xlating_fir::XlatingFir;
pub use xlating_fir::XlatingFirBuilder;
#[cfg(feature = "zynq")]
pub use zynq::Zynq;
#[cfg(feature = "zynq")]
pub use zynq_sync::ZynqSync;

#[cfg(feature = "wgpu")]
pub use self::wgpu::Wgpu;

mod apply;
mod applynm;
mod applyintoiter;
pub mod audio;

#[cfg(not(target_arch = "wasm32"))]
mod blob_to_udp;
mod channel_source;
mod channel_sink;
mod combine;
mod console_sink;
mod copy;
mod copy_rand;
mod delay;
mod filter;
mod fir;
mod fft;
#[cfg(not(target_arch = "wasm32"))]
mod file_sink;
#[cfg(not(target_arch = "wasm32"))]
mod file_source;
mod finite_source;
mod head;
mod iir;
#[cfg(feature = "lttng")]
pub mod lttng;

mod message_annotator;
mod message_burst;
mod message_copy;
mod message_pipe;
mod message_sink;
#[cfg(not(target_arch = "wasm32"))]
mod message_source;
mod null_sink;
mod null_source;
mod pfb;
/// Seify hardware driver blocks
#[cfg(feature = "seify")]
pub mod seify;

mod selector;
pub mod signal_source;
mod sink;
mod source;
mod split;
mod stream_deinterleaver;
mod stream_duplicator;
mod tag_debug;
#[cfg(not(target_arch = "wasm32"))]
mod tcp_sink;
#[cfg(not(target_arch = "wasm32"))]
mod tcp_source;
mod throttle;
#[cfg(not(target_arch = "wasm32"))]
mod udp_source;
mod vector_sink;
mod vector_source;
#[cfg(feature = "vulkan")]
mod vulkan;
/// WASM-specfici blocks (target wasm32-unknown-unknown)
#[cfg(target_arch = "wasm32")]
pub mod wasm;

#[cfg(not(target_arch = "wasm32"))]
mod websocket_sink;
#[cfg(not(target_arch = "wasm32"))]
mod websocket_pmt_sink;
#[cfg(feature = "wgpu")]
mod wgpu;
pub mod xlating_fir;
#[cfg(feature = "zeromq")]
pub mod zeromq;

#[cfg(feature = "zynq")]
mod zynq;
#[cfg(feature = "zynq")]
mod zynq_sync;


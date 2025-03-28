use std::sync::Arc;

use rustfft::Fft;
use rustfft::FftDirection;
use rustfft::FftPlanner;

use futuredsp::Filter;
use futuredsp::FirFilter;

use crate::num_complex::Complex32;
use crate::runtime::BlockMeta;
use crate::runtime::BlockMetaBuilder;
use crate::runtime::Kernel;
use crate::runtime::MessageIo;
use crate::runtime::MessageIoBuilder;
use crate::runtime::Result;
use crate::runtime::StreamIo;
use crate::runtime::StreamIoBuilder;
use crate::runtime::TypedBlock;
use crate::runtime::WorkIo;

use super::utilities::partition_filter_taps;
use super::window_buffer::WindowBuffer;

/// Polyphase Synthesizer.
pub struct PfbSynthesizer {
    num_channels: usize,
    ifft: Arc<dyn Fft<f32>>,
    fft_buf: Vec<Complex32>,
    fir_filters: Vec<FirFilter<Complex32, Complex32, Vec<f32>>>,
    window_buf: Vec<WindowBuffer>,
    all_windows_filled: bool,
}

impl PfbSynthesizer {
    /// Create Polyphase Synthesizer.
    pub fn new(num_channels: usize, taps: &[f32]) -> TypedBlock<Self> {
        let (partitioned_filters, filter_length) = partition_filter_taps(taps, num_channels);
        let synthesizer = PfbSynthesizer {
            num_channels,
            ifft: FftPlanner::new().plan_fft(num_channels, FftDirection::Inverse),
            fft_buf: vec![Complex32::default(); num_channels],
            fir_filters: partitioned_filters,
            window_buf: vec![WindowBuffer::new(filter_length, false); num_channels],
            all_windows_filled: false,
        };

        let mut sio = StreamIoBuilder::new();
        for i in 0..num_channels {
            sio = sio.add_input::<Complex32>(format!("in{i}").as_str())
        }
        sio = sio.add_output::<Complex32>("out");

        TypedBlock::new(
            BlockMetaBuilder::new("PfbSynthesizer").build(),
            sio.build(),
            MessageIoBuilder::new().build(),
            synthesizer,
        )
    }
}

#[async_trait]
impl Kernel for PfbSynthesizer {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _m: &mut MessageIo<Self>,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        let out = sio.output(0).slice::<Complex32>();
        let inputs: Vec<&[Complex32]> = sio
            .inputs_mut()
            .iter_mut()
            .map(|x| x.slice::<Complex32>())
            .collect();
        let n_items_to_consume = inputs.iter().map(|x| x.len()).min().unwrap();
        let n_items_to_produce = out.len();

        let mut consumed_per_channel: usize = 0;
        let mut produced: usize = 0;
        while n_items_to_consume - consumed_per_channel > 0
            && (n_items_to_produce - produced > self.num_channels || !self.all_windows_filled)
        {
            // interleave input streams, taking self.num_channels samples
            for (input, fft_input_slot) in inputs.iter().zip(self.fft_buf.iter_mut()) {
                *fft_input_slot = input[consumed_per_channel];
            }
            consumed_per_channel += 1;
            // spin through IFFT
            self.ifft.process(&mut self.fft_buf);
            for ((window, fir_filter), spun_sample) in self
                .window_buf
                .iter_mut()
                .zip(self.fir_filters.iter())
                .zip(self.fft_buf.iter())
            {
                window.push(*spun_sample);
                if window.filled() {
                    fir_filter.filter(window.get_as_slice(), &mut out[produced..produced + 1]);
                    produced += 1;
                }
            }
            if !self.all_windows_filled {
                self.all_windows_filled = self.window_buf.iter().all(|w| w.filled());
            }
        }
        if consumed_per_channel > 0 {
            for i in 0..self.num_channels {
                sio.input(i).consume(consumed_per_channel);
            }
            if produced > 0 {
                sio.output(0).produce(produced);
            }
        }
        // each iteration either depletes the available input items or the available space in the out buffer, therefore no manual call_again necessary
        // appropriately propagate flowgraph termination
        let samples_remaining_per_input: Vec<bool> = sio
            .inputs_mut()
            .iter_mut()
            .map(|x| x.slice::<Complex32>())
            .map(|x| x.len() - consumed_per_channel == 0)
            .collect();
        if samples_remaining_per_input
            .iter()
            .zip(sio.inputs().iter().map(|x| x.finished()))
            .any(|(&out_of_samples, finished)| out_of_samples && finished)
        {
            io.finished = true;
        }
        Ok(())
    }
}

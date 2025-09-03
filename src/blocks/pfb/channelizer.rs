/*
 * Derived from the liquid-dsp project.
 * Original copyright and license:
 *
 * Copyright (c) 2007 - 2024 Joseph Gaeddert
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 */

use std::cmp::min;
use std::sync::Arc;

use rustfft::Fft;
use rustfft::FftDirection;
use rustfft::FftPlanner;

use futuredsp::FirFilter;
use futuredsp::prelude::*;

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

fn create_sio_builder(n_filters: usize) -> StreamIoBuilder {
    let mut sio = StreamIoBuilder::new().add_input::<Complex32>("in");
    for i in 0..n_filters {
        sio = sio.add_output::<Complex32>(format!("out{i}").as_str());
    }
    sio
}

/// Polyphase Channelizer
pub struct PfbChannelizer {
    num_channels: usize,
    decimation_factor: usize,
    ifft: Arc<dyn Fft<f32>>,
    fft_buf: Vec<Complex32>,
    fir_filters: Vec<FirFilter<Complex32, Complex32, Vec<f32>>>,
    window_buf: Vec<WindowBuffer>,
    base_index: usize,
    all_windows_filled: bool,
}

impl PfbChannelizer {
    /// Create Polyphase Channelizer.
    pub fn new(num_channels: usize, taps: &[f32], oversample_rate: f32) -> TypedBlock<Self> {
        // validate input
        assert!(
            num_channels > 2,
            "PfbChannelizer: number of channels must be at least 2"
        );
        assert!(
            taps.len() >= num_channels,
            "PfbChannelizer: prototype filter length must be at least num_channels"
        );
        assert!(
            oversample_rate != 0. && num_channels as f32 % oversample_rate == 0.,
            "pfb_channelizer: oversample rate must be N/i for i in [1, N]"
        );

        let decimation_factor = (num_channels as f32 / oversample_rate) as usize;
        let (partitioned_filters, filter_semi_length) = partition_filter_taps(taps, num_channels);

        let channelizer = PfbChannelizer {
            num_channels,
            decimation_factor,
            ifft: FftPlanner::new().plan_fft(num_channels, FftDirection::Inverse),
            fft_buf: vec![Complex32::default(); num_channels],
            fir_filters: partitioned_filters,
            window_buf: vec![WindowBuffer::new(filter_semi_length, false); num_channels],
            base_index: num_channels - 1,
            all_windows_filled: false,
        };

        let sio = create_sio_builder(num_channels);

        TypedBlock::new(
            BlockMetaBuilder::new("PfbChannelizer").build(),
            sio.build(),
            MessageIoBuilder::new().build(),
            channelizer,
        )
    }

    fn decrement_base_index(&mut self) {
        // decrement base index, wrapping around
        self.base_index = if self.base_index == 0 {
            self.num_channels - 1
        } else {
            self.base_index - 1
        };
    }
}

#[async_trait]
impl Kernel for PfbChannelizer {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _m: &mut MessageIo<Self>,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        let input = sio.input(0).slice::<Complex32>();
        let n_items_to_consume = input.len();
        let mut outs: Vec<&mut [Complex32]> = sio
            .outputs_mut()
            .iter_mut()
            .map(|x| x.slice::<Complex32>())
            .collect();
        let n_items_producible = outs.iter().map(|x| x.len()).min().unwrap();
        let n_items_to_produce_per_channel = min(
            n_items_producible,
            n_items_to_consume / self.decimation_factor,
        );
        // fill the sample windows if we do not yet have sufficient history to produce output
        if !self.all_windows_filled {
            let mut consumed = 0;
            // consume as many input samples as the buffer holds or until all windows are filled
            while !self.window_buf.iter().all(|w| w.filled()) {
                if consumed == n_items_to_consume {
                    sio.input(0).consume(consumed);
                    if sio.input(0).finished() {
                        io.finished = true;
                    }
                    return Ok(());
                }
                self.window_buf[self.base_index].push(input[consumed]);
                self.decrement_base_index();
                consumed += 1;
            }
            // all windows are filled, possibly call again if still samples left in input
            self.all_windows_filled = true;
            if n_items_to_consume >= self.decimation_factor {
                io.call_again = true;
            } else if sio.input(0).finished() {
                io.finished = true;
            }
            return Ok(());
        }
        // produce one sample per output stream in each iteration
        for output_sample_index in 0..n_items_to_produce_per_channel {
            // consume only self.decimation_factor new samples to achieve oversampling
            for j in 0..self.decimation_factor {
                // push sample into next buffer where we left of in the last iteration
                self.window_buf[self.base_index]
                    .push(input[output_sample_index * self.decimation_factor + j]);
                self.decrement_base_index();
            }
            // execute filter outputs
            for i in 0..self.num_channels {
                // match filter index to window and (reversed) output index
                let buffer_index = (self.base_index + i + 1) % self.num_channels;
                // execute fir filter
                self.fir_filters[i].filter(
                    self.window_buf[buffer_index].get_as_slice(),
                    &mut self.fft_buf[buffer_index..buffer_index + 1],
                );
            }
            // de-spin through IFFT
            self.ifft.process(&mut self.fft_buf);
            // Send to output channels
            #[allow(clippy::needless_range_loop)]
            for channel_index in 0..self.num_channels {
                outs[channel_index][output_sample_index] = self.fft_buf[channel_index];
            }
        }
        // commit sio buffers
        sio.input(0)
            .consume(n_items_to_produce_per_channel * self.decimation_factor);
        for i in 0..self.num_channels {
            sio.output(i).produce(n_items_to_produce_per_channel);
        }
        // each iteration either depletes the available input items or the available space in the out buffer, therefore no manual call_again necessary
        // appropriately propagate flowgraph termination
        if n_items_to_consume - n_items_to_produce_per_channel * self.decimation_factor
            < self.decimation_factor
            && sio.input(0).finished()
        {
            io.finished = true;
        }
        Ok(())
    }
}

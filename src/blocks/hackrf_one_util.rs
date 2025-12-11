use crate::blocks::BurstPad;
use crate::blocks::BurstSizeRewriter;
use crate::blocks::BurstSplit;
use crate::prelude::*;
use std::collections::VecDeque;

// const HACKRF_BLOCK_SIZE: usize = 12288;
const HACKRF_BLOCK_SIZE: usize = 131072;

impl<I, O> BurstPad<Complex32, I, O>
where
    I: CpuBufferReader<Item = Complex32>,
    O: CpuBufferWriter<Item = Complex32>,
{
    /// Create [`struct@BurstPad`] block specifically to pad sample streams for the HackRF One in half duplex mode, which seems to discard up to 4 buffers worth of TX samples when switching back to RX mode
    pub fn new_for_hackrf() -> Self {
        Self::new(0, 4 * HACKRF_BLOCK_SIZE, Complex32::new(0.0, 0.0))
    }
}

impl<I, O> BurstSizeRewriter<Complex32, I, O>
where
    I: CpuBufferReader<Item = Complex32>,
    O: CpuBufferWriter<Item = Complex32>,
{
    /// Create [`struct@BurstSizeRewriter`] block specifically to pad sample streams for the HackRF One to multiples of the transmit buffer, to ensure immediate and unbroken transmission of bursts
    pub fn new_for_hackrf() -> Self {
        let burst_size_function = |input_size_max_items: usize| {
            ((input_size_max_items as f64 / HACKRF_BLOCK_SIZE as f64).ceil() as usize).max(1)
                * HACKRF_BLOCK_SIZE
        };
        let default_value_generator = || Complex32::default();
        BurstSizeRewriter::<Complex32, I, O>::new(burst_size_function, default_value_generator)
    }
}

impl<I, O> BurstSplit<Complex32, I, O>
where
    I: CpuBufferReader<Item = Complex32>,
    O: CpuBufferWriter<Item = Complex32>,
{
    /// Create [`struct@BurstSplit`] block specifically to chunk sample streams for the HackRF One into bursts of at most [`HACKRF_BLOCK_SIZE`] samples, as the soapy HackRF driver always sends individual chunks of this size, and is not compatible with larger bursts
    pub fn new_for_hackrf() -> Self {
        let burst_size_function = |mut burst_size_original: usize| {
            let mut new_burst_sizes = VecDeque::with_capacity(
                (burst_size_original as f32 / HACKRF_BLOCK_SIZE as f32).ceil() as usize,
            );
            while burst_size_original > 0 {
                let next_burst_size = HACKRF_BLOCK_SIZE.min(burst_size_original);
                new_burst_sizes.push_back(next_burst_size);
                burst_size_original -= next_burst_size;
            }
            new_burst_sizes
        };
        BurstSplit::<Complex32, I, O>::new(burst_size_function)
    }
}

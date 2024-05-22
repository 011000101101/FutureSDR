use rand_distr::num_traits::Zero;
use futuresdr::anyhow::Result;
use futuresdr::macros::async_trait;
use futuresdr::num_complex::{Complex32, Complex64};
use futuresdr::runtime::Block;
use futuresdr::runtime::BlockMeta;
use futuresdr::runtime::BlockMetaBuilder;
use futuresdr::runtime::Kernel;
use futuresdr::runtime::MessageIo;
use futuresdr::runtime::MessageIoBuilder;
use futuresdr::runtime::StreamIo;
use futuresdr::runtime::StreamIoBuilder;
use futuresdr::runtime::WorkIo;

pub struct DemoBlockEstimateDCOffset{
    running_mean_i: f64,
    running_mean_q: f64,
    sample_count: usize,
    last_report: usize,
}

impl DemoBlockEstimateDCOffset {
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> Block {
        Block::new(
            BlockMetaBuilder::new("DemoBlockEstimateDCOffset").build(),
            StreamIoBuilder::new()
                .add_input::<Complex32>("in")
                .add_output::<Complex32>("out")
                .build(),
            MessageIoBuilder::new().build(),
            Self {
                running_mean_i: 0.0,
                running_mean_q: 0.0,
                sample_count: 0,
                last_report: 0
            },
        )
    }
}

#[async_trait]
impl Kernel for DemoBlockEstimateDCOffset {
    async fn work(
        &mut self,
        io: &mut WorkIo,
        sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
    ) -> Result<()> {
        let input = sio.input(0).slice::<Complex32>();
        let output = sio.output(0).slice::<Complex32>();

        let n = std::cmp::min(input.len(), output.len());

        for i in 0..n {
            let sample: Complex32 = input[i];
            if !sample.is_zero() {
                self.sample_count += 1;
                self.running_mean_i += (sample.re as f64 - self.running_mean_i) / self.sample_count as f64;
                self.running_mean_q += (sample.im as f64 - self.running_mean_q) / self.sample_count as f64;
            }
            output[i] = sample;
        }

        if self.sample_count / 10000 > self.last_report {
            println!("IQ imbalance currently at: [{}, {}] ({} samples)", self.running_mean_i, self.running_mean_q, self.sample_count);
            self.last_report = self.sample_count / 10000;
        }

        if sio.input(0).finished(){
            io.finished = true;
        }

        sio.input(0).consume(n);
        sio.output(0).produce(n);

        Ok(())
    }
}

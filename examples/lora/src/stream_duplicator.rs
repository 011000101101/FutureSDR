use futuresdr::anyhow::Result;
use futuresdr::macros::async_trait;
use futuresdr::runtime::Block;
use futuresdr::runtime::BlockMeta;
use futuresdr::runtime::BlockMetaBuilder;
use futuresdr::runtime::Kernel;
use futuresdr::runtime::MessageIo;
use futuresdr::runtime::MessageIoBuilder;
use futuresdr::runtime::StreamIo;
use futuresdr::runtime::StreamIoBuilder;
use futuresdr::runtime::WorkIo;
use std::cmp::min;
use std::marker::PhantomData;

pub struct StreamDuplicator<'a, T> {
    num_out: usize, // number of output streams
    phantom: PhantomData<&'a T>,
}

impl<T> StreamDuplicator<'_, T>
where
    T: Copy + Sync + 'static,
{
    pub fn new(num_outputs: usize) -> Block {
        let mut sio = StreamIoBuilder::new().add_input::<T>("in");
        for i in 0..num_outputs {
            sio = sio.add_output::<T>(&format!("out{}", i));
        }
        Block::new(
            BlockMetaBuilder::new("StreamDuplicator").build(),
            sio.build(),
            MessageIoBuilder::new().build(),
            StreamDuplicator::<'static, T> {
                num_out: num_outputs,
                phantom: PhantomData,
            },
        )
        // set_tag_propagation_policy(TPP_ONE_TO_ONE); // TODO
    }
}

#[async_trait]
impl<T: Copy + Sync + 'static> Kernel for StreamDuplicator<'_, T> {
    async fn work(
        &mut self,
        _io: &mut WorkIo,
        sio: &mut StreamIo,
        _mio: &mut MessageIo<Self>,
        _b: &mut BlockMeta,
    ) -> Result<()> {
        let input = sio.input(0).slice::<T>();
        let nitem_to_consume = input.len();
        let n_items_to_produce = sio
            .outputs_mut()
            .iter_mut()
            .map(|x| x.slice::<T>().len())
            .min()
            .unwrap();
        let nitem_to_process = min(n_items_to_produce, nitem_to_consume);
        if nitem_to_process > 0 {
            for j in 0..self.num_out {
                let out = sio.output(j).slice::<T>();
                out[..nitem_to_process].copy_from_slice(&input[..nitem_to_process]);
                sio.output(j).produce(nitem_to_process);
            }
            sio.input(0).consume(nitem_to_process);
        }
        Ok(())
    }
}

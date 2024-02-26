use futuresdr::anyhow::Result;
use futuresdr::macros::async_trait;
use futuresdr::macros::message_handler;
use futuresdr::runtime::Block;
use futuresdr::runtime::BlockMeta;
use futuresdr::runtime::BlockMetaBuilder;
use futuresdr::runtime::Kernel;
use futuresdr::runtime::MessageIo;
use futuresdr::runtime::MessageIoBuilder;
use futuresdr::runtime::Pmt;
use futuresdr::runtime::StreamIoBuilder;
use futuresdr::runtime::WorkIo;

pub struct MessageSelector {
    current_output: usize,
}

impl MessageSelector {
    pub fn new() -> Block {
        Block::new(
            BlockMetaBuilder::new("MessageSelector").build(),
            StreamIoBuilder::new().build(),
            MessageIoBuilder::new()
                .add_input("message_in", Self::message_in)
                .add_input("output_selector", Self::select_output)
                .add_output("out0")
                .add_output("out1")
                .build(),
            MessageSelector { current_output: 0 },
        )
    }

    #[message_handler]
    async fn message_in(
        &mut self,
        _io: &mut WorkIo,
        mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        mio.output_mut(self.current_output).post(p).await;
        Ok(Pmt::Null)
    }

    #[message_handler]
    async fn select_output(
        &mut self,
        _io: &mut WorkIo,
        _mio: &mut MessageIo<Self>,
        _meta: &mut BlockMeta,
        p: Pmt,
    ) -> Result<Pmt> {
        match p {
            Pmt::U32(v) => self.current_output = (v as usize) % 2,
            x => {
                dbg!(x);
            }
        }
        Ok(Pmt::U32(self.current_output as u32))
    }
}

#[async_trait]
impl Kernel for MessageSelector {}

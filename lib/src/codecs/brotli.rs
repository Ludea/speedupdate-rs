use std::io::{self, Write};

pub use brotli::CompressorWriter;
pub use brotli::DecompressorWriter;

use super::Coder;

impl<W: Write> Coder<W> for DecompressorWriter<W> {
    fn get_mut(&mut self) -> &mut W {
        DecompressorWriter::get_mut(self)
    }

    fn finish(mut self) -> io::Result<W> {
        self.flush()?;
        DecompressorWriter::finish(self)
            .map_err(|_| io::Error::other("brotli decoder failed to finalize stream"))
        //Ok(DecompressorWriter::into_inner(self))
    }

    fn finish_boxed(self: Box<Self>) -> io::Result<W> {
        self.finish()
    }
}

impl<W: Write> Coder<W> for CompressorWriter<W> {
    fn get_mut(&mut self) -> &mut W {
        CompressorWriter::get_mut(self)
    }

    fn finish(mut self) -> io::Result<W> {
        self.flush()?;
        Ok(CompressorWriter::into_inner(self))
    }

    fn finish_boxed(self: Box<Self>) -> io::Result<W> {
        self.finish()
    }
}

//! Stream ZIP output from a dedicated thread to an async runtime through a bounded channel.
//!
//! `ZipWriter::new_stream` only needs `Write`, so it can run on a blocking thread. A bounded
//! channel applies backpressure to the writer, which keeps the archive payload out of memory as a
//! whole. In an async runtime, replace the receiving loop with `recv().await` and use the
//! runtime's bounded channel.

use std::error::Error;
use std::fs::File;
use std::io::{self, Write};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// A `Write` adapter that forwards fixed-size chunks to a bounded channel.
struct ChannelWriter {
    sender: SyncSender<Vec<u8>>,
    buffer: Vec<u8>,
    chunk_size: usize,
}

impl ChannelWriter {
    fn new(sender: SyncSender<Vec<u8>>, chunk_size: usize) -> Self {
        let chunk_size = chunk_size.max(1);
        Self {
            sender,
            buffer: Vec::with_capacity(chunk_size),
            chunk_size,
        }
    }

    fn send_buffer(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::replace(&mut self.buffer, Vec::with_capacity(self.chunk_size));
        self.sender
            .send(chunk)
            .map_err(|_| io::Error::other("ZIP consumer stopped"))
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, mut data: &[u8]) -> io::Result<usize> {
        let written = data.len();
        while !data.is_empty() {
            let room = self.chunk_size - self.buffer.len();
            let take = room.min(data.len());
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buffer.len() == self.chunk_size {
                self.send_buffer()?;
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send_buffer()
    }
}

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "async_bridge.zip".to_string());
    let mut output = File::create(&path)?;

    // At most four chunks are queued. When the consumer is slower than the writer, the writer
    // blocks in `send`, which is the backpressure that keeps memory bounded.
    let (sender, receiver) = sync_channel(4);
    let writer = thread::spawn(move || -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut zip = ZipWriter::new_stream(ChannelWriter::new(sender, 64 * 1024));
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("hello.txt", options)?;
        zip.write_all(b"hello from a streaming zip\n")?;
        zip.start_file("second.txt", options)?;
        zip.write_all(b"another entry\n")?;
        let mut output = zip.finish()?.into_inner();
        output.flush()?;
        Ok(())
    });

    let total = consume(receiver, &mut output)?;
    writer
        .join()
        .map_err(|_| io::Error::other("ZIP writer thread panicked"))??;
    println!("wrote {total} bytes to {path}");
    Ok(())
}

fn consume(receiver: Receiver<Vec<u8>>, output: &mut impl Write) -> io::Result<usize> {
    let mut total = 0;
    for chunk in receiver {
        // In an async runtime this loop would be `while let Some(chunk) = rx.recv().await`, and
        // the write would be `output.write_all(&chunk).await`.
        output.write_all(&chunk)?;
        total += chunk.len();
    }
    Ok(total)
}

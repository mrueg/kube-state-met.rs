//! Parallel gzip for /metrics (pigz style). The body is cut into the chunks the
//! writer already produces; each chunk is deflated on its own rayon thread,
//! primed with the 32 KiB of data before it as the dictionary, and ends in a
//! sync flush, so the pieces join into one ordinary deflate stream. The output
//! is a single standard gzip member (RFC 1952): any client reads it, and the
//! ratio is within a fraction of a percent of compressing the body in one pass.
//!
//! Compressing in parallel costs CPU: on a laptop with SMT, eight chunks at
//! once took 1.7x the CPU time of one after another (shared cores, lower
//! clocks). It pays only when cores are idle, so the width is shared out
//! between the gzip responses in flight ([`Width`]): a lone scrape gets half
//! the pool, concurrent scrapes compress serially on their own threads.

use flate2::{Compress, Compression, FlushCompress};
use rayon::prelude::*;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};

/// zlib level 5. Go's `gzip.DefaultCompression` is level 6 of its own
/// compress/flate, which spends 0.53s on a 10k-pod body (83 MB); zlib-rs at 6
/// spends 0.65s and at 5 spends 0.47s, for output 0.4% larger than its 6 and
/// 1.8% larger than Go's.
pub const LEVEL: Compression = Compression::new(5);

/// Most chunks compressed at once by one response.
const MAX_WIDTH: usize = 8;

static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// One gzip response's share of the compression threads, held while it runs.
pub struct Width(usize);

impl Width {
    /// Half the rayon pool (the physical cores on an SMT machine), split
    /// between this and the other gzip responses in flight.
    pub fn take() -> Self {
        let others = IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
        let cores = (rayon::current_num_threads() / 2).max(1);
        Width((cores / (others + 1)).clamp(1, MAX_WIDTH))
    }

    pub fn get(&self) -> usize {
        self.0
    }
}

impl Drop for Width {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Deflate's window: a chunk can refer back this far into the previous data.
const WINDOW: usize = 32 * 1024;

/// gzip header: magic, deflate, no flags, no mtime, no extra flags, OS unknown
/// (what Go's compress/gzip writes).
const HEADER: [u8; 10] = [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];

pub struct ParallelGzip<F: FnMut(Vec<u8>) -> io::Result<()>> {
    level: Compression,
    /// Raw chunks waiting to be compressed together.
    pending: Vec<Vec<u8>>,
    /// The last WINDOW bytes of the data before `pending`.
    dict: Vec<u8>,
    crc: crc32fast::Hasher,
    len: u64,
    header_sent: bool,
    batch: usize,
    sink: F,
}

impl<F: FnMut(Vec<u8>) -> io::Result<()>> ParallelGzip<F> {
    /// Compresses up to `batch` chunks at a time and hands compressed output to
    /// `sink` in order; a sink error (the reader went away) stops the stream.
    pub fn new(level: Compression, batch: usize, sink: F) -> Self {
        ParallelGzip {
            level,
            pending: Vec::new(),
            dict: Vec::new(),
            crc: crc32fast::Hasher::new(),
            len: 0,
            header_sent: false,
            batch: batch.max(1),
            sink,
        }
    }

    pub fn push(&mut self, chunk: Vec<u8>) -> io::Result<()> {
        if chunk.is_empty() {
            return Ok(());
        }
        self.pending.push(chunk);
        if self.pending.len() >= self.batch {
            self.compress_pending()?;
        }
        Ok(())
    }

    fn compress_pending(&mut self) -> io::Result<()> {
        let chunks = std::mem::take(&mut self.pending);
        let mut dicts = Vec::with_capacity(chunks.len());
        for c in &chunks {
            let next = tail(&self.dict, c);
            dicts.push(std::mem::replace(&mut self.dict, next));
        }
        let level = self.level;
        let one = |(c, d): (&Vec<u8>, &Vec<u8>)| {
            let mut h = crc32fast::Hasher::new();
            h.update(c);
            Ok((deflate_chunk(level, d, c, FlushCompress::Sync)?, h))
        };
        // A single chunk is compressed on this thread: handing it to the pool
        // would only add a wakeup and a wait.
        let parts: Vec<io::Result<(Vec<u8>, crc32fast::Hasher)>> = if chunks.len() == 1 {
            chunks.iter().zip(&dicts).map(one).collect()
        } else {
            chunks.par_iter().zip(dicts.par_iter()).map(one).collect()
        };
        self.send_header()?;
        for (part, c) in parts.into_iter().zip(&chunks) {
            let (data, h) = part?;
            self.crc.combine(&h);
            self.len += c.len() as u64;
            (self.sink)(data)?;
        }
        Ok(())
    }

    fn send_header(&mut self) -> io::Result<()> {
        if self.header_sent {
            return Ok(());
        }
        self.header_sent = true;
        (self.sink)(HEADER.to_vec())
    }

    /// Compresses what is left, then ends the stream (final block and trailer).
    pub fn finish(mut self) -> io::Result<()> {
        self.compress_pending()?;
        self.send_header()?;
        let mut end = deflate_chunk(self.level, &[], &[], FlushCompress::Finish)?;
        end.extend_from_slice(&self.crc.clone().finalize().to_le_bytes());
        // ISIZE is the length modulo 2^32.
        end.extend_from_slice(&(self.len as u32).to_le_bytes());
        (self.sink)(end)
    }
}

/// The last WINDOW bytes of `prev` followed by `chunk`.
fn tail(prev: &[u8], chunk: &[u8]) -> Vec<u8> {
    if chunk.len() >= WINDOW {
        return chunk[chunk.len() - WINDOW..].to_vec();
    }
    let keep = (WINDOW - chunk.len()).min(prev.len());
    let mut t = Vec::with_capacity(keep + chunk.len());
    t.extend_from_slice(&prev[prev.len() - keep..]);
    t.extend_from_slice(chunk);
    t
}

/// Raw deflate of `data` with `dict` as the preset window, ending with `flush`
/// (Sync: byte aligned, not final; Finish: final block).
fn deflate_chunk(
    level: Compression,
    dict: &[u8],
    data: &[u8],
    flush: FlushCompress,
) -> io::Result<Vec<u8>> {
    let mut c = Compress::new(level, false);
    if !dict.is_empty() {
        c.set_dictionary(dict).map_err(io::Error::other)?;
    }
    let mut out = Vec::with_capacity(data.len() / 4 + 64);
    let mut pos = 0;
    loop {
        if out.capacity() - out.len() < 64 {
            out.reserve(out.capacity().max(4096));
        }
        let before = c.total_in();
        c.compress_vec(&data[pos..], &mut out, flush)
            .map_err(io::Error::other)?;
        pos += usize::try_from(c.total_in() - before).map_err(io::Error::other)?;
        // zlib's rule: the flush is complete once it leaves output space unused.
        if pos == data.len() && out.len() < out.capacity() {
            return Ok(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn roundtrip(chunks: &[Vec<u8>], batch: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut gz = ParallelGzip::new(Compression::default(), batch, |b| {
            out.extend_from_slice(&b);
            Ok(())
        });
        for c in chunks {
            gz.push(c.clone()).unwrap();
        }
        gz.finish().unwrap();
        let mut dec = Vec::new();
        // A single-member decoder: concatenated members would fail here.
        flate2::read::GzDecoder::new(&out[..])
            .read_to_end(&mut dec)
            .unwrap();
        dec
    }

    fn sample(n: usize, seed: u64) -> Vec<u8> {
        let mut s = seed;
        let mut v = Vec::with_capacity(n);
        while v.len() < n {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let line = format!(
                "kube_pod_info{{namespace=\"ns-{}\",pod=\"p-{}\"}} 1\n",
                s >> 60,
                s >> 40
            );
            v.extend_from_slice(line.as_bytes());
        }
        v.truncate(n);
        v
    }

    #[test]
    fn width_is_shared_between_responses() {
        let cores = (rayon::current_num_threads() / 2).max(1);
        let a = Width::take();
        assert!(a.get() >= 1 && a.get() <= cores.min(MAX_WIDTH));
        let b = Width::take();
        assert!(
            b.get() <= (cores / 2).max(1),
            "{} with two in flight",
            b.get()
        );
        drop((a, b));
    }

    #[test]
    fn empty_body_is_valid_gzip() {
        assert!(roundtrip(&[], 4).is_empty());
    }

    #[test]
    fn chunks_join_into_one_stream() {
        for batch in [1, 2, 3, 16] {
            let chunks: Vec<Vec<u8>> = [0, 10, 40_000, 1 << 20, 7, 100_000, 31_000, 33_000]
                .iter()
                .enumerate()
                .map(|(i, &n)| sample(n, i as u64))
                .collect();
            assert_eq!(roundtrip(&chunks, batch), chunks.concat(), "batch {batch}");
        }
    }

    #[test]
    fn ratio_is_close_to_single_pass() {
        let chunks: Vec<Vec<u8>> = (0..8).map(|i| sample(1 << 20, i)).collect();
        let mut par = Vec::new();
        let mut gz = ParallelGzip::new(Compression::default(), 8, |b| {
            par.extend_from_slice(&b);
            Ok(())
        });
        for c in &chunks {
            gz.push(c.clone()).unwrap();
        }
        gz.finish().unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), Compression::default());
        std::io::Write::write_all(&mut enc, &chunks.concat()).unwrap();
        let single = enc.finish().unwrap();
        assert!(
            par.len() as f64 <= single.len() as f64 * 1.01,
            "{} vs {}",
            par.len(),
            single.len()
        );
    }
}

//! Writes data to file/stdout. Uses a buffer to reduce the amount of syscalls.
//!
//! Receives data incrementally and once a certain amount is reached, it will
//! write it out to file/stdout.
//! Implements drop to flush the remaining data to the file once processing is done.

use crate::input::data_wrapper::CdpChunk;
use crate::util::lib::Config;
use crate::words::lib::RDH;

/// Trait for a writer that can write ALICE readout data to file/stdout.
pub trait Writer<T: RDH> {
    /// Write data to file/stdout
    fn write(&mut self, data: &[u8]) -> std::io::Result<()>;
    /// Push a vector of RDHs to the buffer
    fn push_rdhs(&mut self, rdhs: Vec<T>);
    /// Push a vector of payloads to the buffer
    fn push_payload(&mut self, payload: Vec<u8>);
    /// Push a CDP chunk to the buffer
    fn push_cdp_chunk(&mut self, cdp_chunk: CdpChunk<T>);
    /// Flush the buffer to file/stdout
    fn flush(&mut self) -> std::io::Result<()>;
}

/// A writer that uses a buffer to reduce the amount of syscalls.
pub struct BufferedWriter<T: RDH> {
    filtered_rdhs_buffer: Vec<T>,
    filtered_payload_buffers: Vec<Vec<u8>>, // 1 Linked list per payload
    buf_writer: Option<std::io::BufWriter<std::fs::File>>, // If no file is specified -> write to stdout
    max_buffer_size: usize,
}

impl<T: RDH> BufferedWriter<T> {
    /// Create a new BufferedWriter from a config and a max buffer size.
    pub fn new(config: &impl Config, max_buffer_size: usize) -> Self {
        // Create output file, and buf writer if specified
        let buf_writer = match config.output() {
            Some(path) if "stdout".eq(path.to_str().unwrap()) => None,
            Some(path) => {
                let path: std::path::PathBuf = path.to_owned();
                // Likely better to use File::create_new() but it's not stable yet
                let mut _f = std::fs::File::create(&path).expect("Failed to create output file");
                let file = std::fs::File::options()
                    .append(true)
                    .open(path)
                    .expect("Failed to open/create output file");
                let buf_writer = std::io::BufWriter::new(file);
                Some(buf_writer)
            }
            None => None,
        };
        BufferedWriter {
            filtered_rdhs_buffer: Vec::with_capacity(max_buffer_size), // Will most likely not be filled as payloads are usually larger, but hard to say
            filtered_payload_buffers: Vec::with_capacity(max_buffer_size),
            buf_writer,
            max_buffer_size,
        }
    }
}

impl<T: RDH> Writer<T> for BufferedWriter<T> {
    #[inline]
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        match &mut self.buf_writer {
            Some(buf_writer) => std::io::Write::write_all(buf_writer, data),
            None => std::io::Write::write_all(&mut std::io::stdout(), data),
        }
    }

    #[inline]
    fn push_rdhs(&mut self, rdhs: Vec<T>) {
        if self.filtered_rdhs_buffer.len() + rdhs.len() >= self.max_buffer_size {
            self.flush().expect("Failed to flush buffer");
        }
        self.filtered_rdhs_buffer.extend(rdhs);
    }

    #[inline]
    fn push_payload(&mut self, payload: Vec<u8>) {
        if self.filtered_payload_buffers.len() + 1 >= self.max_buffer_size {
            self.flush().expect("Failed to flush buffer");
        }
        self.filtered_payload_buffers.push(payload);
    }

    #[inline]
    fn push_cdp_chunk(&mut self, cdp_chunk: CdpChunk<T>) {
        if (self.filtered_rdhs_buffer.len() + cdp_chunk.len() >= self.max_buffer_size)
            || (self.filtered_payload_buffers.len() + cdp_chunk.len() >= self.max_buffer_size)
        {
            self.flush().expect("Failed to flush buffer");
        }
        cdp_chunk.into_iter().for_each(|(rdh, payload, _)| {
            self.filtered_rdhs_buffer.push(rdh);
            self.filtered_payload_buffers.push(payload);
        });
    }

    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        debug_assert_eq!(
            self.filtered_rdhs_buffer.len(),
            self.filtered_payload_buffers.len()
        );

        let mut data = vec![];
        for (rdh, payload) in self
            .filtered_rdhs_buffer
            .iter()
            .zip(self.filtered_payload_buffers.iter())
        {
            data.extend(rdh.to_byte_slice());
            data.extend(payload);
        }

        self.write(&data)?;
        self.filtered_rdhs_buffer.clear();
        self.filtered_payload_buffers.clear();
        Ok(())
    }
}

impl<T: RDH> Drop for BufferedWriter<T> {
    fn drop(&mut self) {
        if std::mem::needs_drop::<Self>() {
            self.flush().expect("Failed to flush buffer");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::vec;

    use crate::util::config::Opt;
    use crate::words::rdh_cru::test_data::CORRECT_RDH_CRU_V7;
    use crate::words::rdh_cru::{RdhCRU, V6, V7};

    use super::*;

    const OUTPUT_FILE_STR: &str = " test_filter_link.raw";
    const OUTPUT_CMD: &str = "-o test_filter_link.raw";
    const CONFIG_STR: [&str; 7] = [
        "fastpasta",
        "../fastpasta_test_files/data_ols_ul.raw",
        OUTPUT_CMD,
        "-f",
        "2",
        "check",
        "sanity",
    ];

    #[test]
    fn test_buffered_writer() {
        let config: Opt = <Opt as structopt::StructOpt>::from_iter(&CONFIG_STR);
        {
            let writer = BufferedWriter::<RdhCRU<V6>>::new(&config, 10);

            assert!(writer.buf_writer.is_some());
        }

        let filepath = std::path::PathBuf::from(OUTPUT_FILE_STR);

        // delete output file
        std::fs::remove_file(filepath).unwrap();
    }

    #[test]
    #[should_panic]
    // Should panic, Because when the writer is dropped, it flushes the buffer, which will panic because the number of RDHs and payloads are not equal
    // Empty payloads are counted.
    fn test_push_2_rdh_v7_buffer_is_2() {
        let config: Opt = <Opt as structopt::StructOpt>::from_iter(&CONFIG_STR);
        let rdhs = vec![CORRECT_RDH_CRU_V7, CORRECT_RDH_CRU_V7];
        let length = rdhs.len();
        println!("length: {}", length);
        {
            let mut writer = BufferedWriter::<RdhCRU<V7>>::new(&config, 10);
            writer.push_rdhs(rdhs);
            let buf_size = writer.filtered_rdhs_buffer.len();
            println!("buf_size: {}", buf_size);
            assert_eq!(buf_size, length);
            // Clean up before drop
            let filepath = std::path::PathBuf::from(OUTPUT_FILE_STR);
            // delete output file
            std::fs::remove_file(filepath).unwrap();
        }
    }

    #[test]
    fn test_push_2_rdh_v7_and_empty_payloads_buffers_are_2() {
        let config: Opt = <Opt as structopt::StructOpt>::from_iter(&CONFIG_STR);
        let mut cdp_chunk = CdpChunk::new();

        cdp_chunk.push(CORRECT_RDH_CRU_V7, vec![0; 10], 0);
        cdp_chunk.push(CORRECT_RDH_CRU_V7, vec![0; 10], 0x40);

        let length = cdp_chunk.len();
        {
            let mut writer = BufferedWriter::<RdhCRU<V7>>::new(&config, 10);
            writer.push_cdp_chunk(cdp_chunk);
            let buf_size = writer.filtered_rdhs_buffer.len();
            assert_eq!(buf_size, length);
        }

        // CLEANUP
        let filepath = std::path::PathBuf::from(OUTPUT_FILE_STR);
        // delete output file
        std::fs::remove_file(filepath).unwrap();
    }
}

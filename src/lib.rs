use std::{fs::File, io::Write, path::PathBuf};

use data_words::rdh::{RdhCRUv6, RdhCRUv7};

pub mod data_words;
pub mod macros;
pub mod validators;

use structopt::StructOpt;
/// StructOpt is a library that allows parsing command line arguments
#[derive(StructOpt, Debug)]
#[structopt(
    name = "fastPASTA - fast Protocol Analysis Scanning Tool for ALICE",
    about = "A tool to scan and verify the CRU protocol of the ALICE readout system"
)]
pub struct Opt {
    /// Dump RDHs to stdout or file
    #[structopt(short, long = "dump-rhds")]
    dump_rhds: bool,

    /// Activate sanity checks
    #[structopt(short = "s", long = "sanity-checks")]
    sanity_checks: bool,

    /// links to filter
    #[structopt(short = "f", long)]
    filter_link: Option<u8>,

    /// File to process
    #[structopt(name = "FILE", parse(from_os_str))]
    file: PathBuf,

    /// Output file
    #[structopt(short, long, parse(from_os_str))]
    output: Option<PathBuf>,
}

impl Opt {
    pub fn dump_rhds(&self) -> bool {
        self.dump_rhds
    }
    pub fn sanity_checks(&self) -> bool {
        self.sanity_checks
    }
    pub fn filter_link(&self) -> Option<u8> {
        self.filter_link
    }
    pub fn file(&self) -> &PathBuf {
        &self.file
    }
    pub fn output(&self) -> &Option<PathBuf> {
        &self.output
    }
}

/// This is the trait that all GBT words must implement
/// It is used to:
/// * pretty printing to stdout
/// * deserialize the GBT words from the binary file
pub trait GbtWord: std::fmt::Debug {
    fn print(&self);
    fn load<T: std::io::Read>(reader: &mut T) -> Result<Self, std::io::Error>
    where
        Self: Sized;
}

pub trait LoadRdhCru<T> {
    fn load_rdh_cru(&mut self) -> Result<T, std::io::Error>
    where
        T: GbtWord;
}

/// This trait is used to convert a struct to a byte slice
/// All structs that are used to represent a full GBT word (not sub RDH words) must implement this trait
pub trait ByteSlice {
    fn to_byte_slice(&self) -> &[u8];
}

/// # Safety
/// This function can only be used to serialize a struct if it has the #[repr(packed)] attribute
/// If there's any padding on T, it is UNITIALIZED MEMORY and therefor UNDEFINED BEHAVIOR!
#[inline]
pub unsafe fn any_as_u8_slice<T: Sized>(p: &T) -> &[u8] {
    // Create read-only reference to T as a byte slice, safe as long as no padding bytes are read
    ::core::slice::from_raw_parts((p as *const T) as *const u8, ::core::mem::size_of::<T>())
}

#[inline]
pub fn file_open_read_only(path: &PathBuf) -> std::io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .expect("File not found");
    Ok(file)
}

/// Only use
#[inline]
pub fn file_open_append(path: &PathBuf) -> std::io::Result<std::fs::File> {
    let file = File::options().append(true).open(path)?;
    Ok(file)
}

#[inline(always)]
pub fn buf_reader_with_capacity(
    file: std::fs::File,
    capacity: usize,
) -> std::io::BufReader<std::fs::File> {
    std::io::BufReader::with_capacity(capacity, file)
}

pub fn setup_buffered_reading(config: &Opt) -> std::io::BufReader<std::fs::File> {
    const CAPACITY: usize = 1024 * 10; // 10 KB
    let file = file_open_read_only(&config.file()).expect("Failed to open file");
    buf_reader_with_capacity(file, CAPACITY)
}

pub struct FileScanner<'a> {
    pub reader: std::io::BufReader<std::fs::File>,
    pub tracker: &'a mut FilePosTracker,
    pub stats: &'a mut Stats,
}

impl<'a> FileScanner<'a> {
    pub fn new(
        reader: std::io::BufReader<std::fs::File>,
        tracker: &'a mut FilePosTracker,
        stats: &'a mut Stats,
        config: &'a Opt,
    ) -> Self {
        FileScanner {
            reader,
            tracker,
            stats,
        }
    }
}

impl LoadRdhCru<RdhCRUv7> for FileScanner<'_> {
    fn load_rdh_cru(&mut self) -> Result<RdhCRUv7, std::io::Error> {
        let rdh = RdhCRUv7::load(&mut self.reader)?;
        self.tracker.next(rdh.offset_new_packet as u64);
        self.stats.total_rdhs += 1;
        self.stats.payload_size += rdh.offset_new_packet as u64;
        Ok(rdh)
    }
}

impl LoadRdhCru<RdhCRUv6> for FileScanner<'_> {
    fn load_rdh_cru(&mut self) -> Result<RdhCRUv6, std::io::Error> {
        let rdh = RdhCRUv6::load(&mut self.reader)?;
        self.tracker.next(rdh.offset_new_packet as u64);
        self.stats.total_rdhs += 1;
        self.stats.payload_size += rdh.offset_new_packet as u64;
        Ok(rdh)
    }
}

pub struct FilePosTracker {
    pub offset_next: i64,
    pub memory_address_bytes: u64,
    rdh_cru_size_bytes: u64,
}
impl FilePosTracker {
    pub fn new() -> Self {
        FilePosTracker {
            offset_next: 0,
            memory_address_bytes: 0,
            rdh_cru_size_bytes: 64, // RDH size in bytes
        }
    }
    pub fn next(&mut self, rdh_offset: u64) -> i64 {
        self.offset_next = (rdh_offset - self.rdh_cru_size_bytes) as i64;
        self.memory_address_bytes += rdh_offset;
        self.offset_next
    }
}

pub struct Stats {
    pub total_rdhs: u64,
    pub payload_size: u64,
    pub links_observed: Vec<u8>,
    pub processing_time: std::time::Instant,
}
impl Stats {
    pub fn new() -> Self {
        Stats {
            total_rdhs: 0,
            payload_size: 0,
            links_observed: vec![],
            processing_time: std::time::Instant::now(),
        }
    }
    pub fn print(&self) {
        println!("Total RDHs: {}", self.total_rdhs);
        println!("Total payload size: {}", self.payload_size);
        println!("Links observed: {:?}", self.links_observed);
        println!("Processing time: {:?}", self.processing_time.elapsed());
    }
    pub fn print_time(&self) {
        println!("Processing time: {:?}", self.processing_time.elapsed());
    }
}

pub struct FilterLink {
    link_to_filter: u8,
    output: Option<File>, // If no file is specified -> write to stdout
    pub max_buffer_size: usize,
    pub filtered_rdhs_buffer: Vec<RdhCRUv7>,
    pub filtered_payload_buffers: Vec<Vec<u8>>, // 1 Linked list per payload
    total_filtered: u64,
}
impl FilterLink {
    pub fn new(config: &Opt, max_buffer_size: usize) -> Self {
        let f = match config.output() {
            Some(path) => {
                let path: PathBuf = path.to_owned();
                // Likely better to use File::create_new() but it's not stable yet
                let mut _f = File::create(path.to_owned()).expect("Failed to create output file");
                let file = file_open_append(&path).expect("Failed to open output file");
                Some(file)
            }
            None => None,
        };

        FilterLink {
            link_to_filter: config.filter_link().expect("No link to filter specified"),
            output: f,
            filtered_rdhs_buffer: vec![],
            max_buffer_size,
            filtered_payload_buffers: Vec::with_capacity(1024), // 1 KB capacity to prevent frequent reallocations
            total_filtered: 0,
        }
    }
    pub fn filter_link<T: std::io::Read>(&mut self, buf_reader: &mut T, rdh: RdhCRUv7) -> bool {
        if rdh.link_id == self.link_to_filter {
            // Read the payload of the RDH
            self.read_payload(buf_reader, rdh.memory_size as usize)
                .expect("Failed to read from buffer");

            if self.filtered_rdhs_buffer.len() > self.max_buffer_size {
                self.flush();
            }
            self.filtered_rdhs_buffer.push(rdh);
            self.total_filtered += 1;
            true
        } else {
            false
        }
    }
    fn flush(&mut self) {
        if self.filtered_rdhs_buffer.len() > 0 {
            if self.filtered_rdhs_buffer.len() != self.filtered_payload_buffers.len() {
                panic!("Number of RDHs and payloads don't match!");
            }
            if self.output.is_some() {
                // Write RDHs and payloads to file by zip iterator (RDH, payload)
                self.filtered_rdhs_buffer
                    .iter()
                    .zip(self.filtered_payload_buffers.iter())
                    .for_each(|(rdh, payload)| {
                        self.output
                            .as_ref()
                            .unwrap()
                            .write_all(rdh.to_byte_slice())
                            .unwrap();
                        self.output.as_ref().unwrap().write_all(payload).unwrap();
                    });
            } else {
                // Write RDHs and payloads to stdout by zip iterator (RDH, payload)
                self.filtered_rdhs_buffer
                    .iter()
                    .zip(self.filtered_payload_buffers.iter())
                    .for_each(|(rdh, payload)| {
                        std::io::stdout().write_all(rdh.to_byte_slice()).unwrap();
                        std::io::stdout().write_all(payload).unwrap();
                    });
            }
            self.filtered_rdhs_buffer.clear();
            self.filtered_payload_buffers.clear();
        }
    }

    fn read_payload<T: std::io::Read>(
        &mut self,
        buf_reader: &mut T,
        payload_size: usize,
    ) -> Result<(), std::io::Error> {
        let payload_size = payload_size - 64; // RDH size in bytes
        let mut payload: Vec<u8> = vec![0; payload_size];
        buf_reader
            .read_exact(&mut payload)
            .expect("Failed to read payload");
        self.filtered_payload_buffers.push(payload);
        Ok(())
    }
    pub fn print_stats(&self) {
        println!("Total filtered RDHs: {}", self.total_filtered);
    }
}

impl Drop for FilterLink {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_file_tracker() {
        let mut file_tracker = FilePosTracker::new();
        assert_eq!(file_tracker.offset_next, 0);
        assert_eq!(file_tracker.memory_address_bytes, 0);
        assert_eq!(file_tracker.next(64), 0);
        assert_eq!(file_tracker.offset_next, 0);
        assert_eq!(file_tracker.memory_address_bytes, 64);
        assert_eq!(file_tracker.next(64), 0);
        assert_eq!(file_tracker.offset_next, 0);
        assert_eq!(file_tracker.memory_address_bytes, 128);
    }

    #[test]
    fn test_filter_link() {
        let mut config: Opt =
            Opt::from_iter(&["fastpasta", "../fastpasta_test_files/data_ols_ul.raw"]);
        config.dump_rhds = true;
        config.filter_link = Some(0);
        config.output = Some(PathBuf::from("test_filter_link.raw"));
        println!("{:#?}", config);

        let mut filter_link = FilterLink::new(&config, 1024);

        assert_eq!(filter_link.link_to_filter, 0);
        assert_eq!(filter_link.filtered_rdhs_buffer.len(), 0);
        assert_eq!(filter_link.filtered_payload_buffers.len(), 0);

        let file = file_open_read_only(&config.file()).unwrap();
        let mut buf_reader = buf_reader_with_capacity(file, 1024 * 10);
        let mut file_tracker = FilePosTracker::new();
        let rdh = RdhCRUv7::load(&mut buf_reader).unwrap();
        RdhCRUv7::print_header_text();
        rdh.print();
        assert!(filter_link.filter_link(&mut buf_reader, rdh));
        // This function currently corresponds to the unlink function on Unix and the DeleteFile function on Windows. Note that, this may change in the future.
        // More info: https://doc.rust-lang.org/std/fs/fn.remove_file.html
        std::fs::remove_file(Opt::output(&config).as_ref().unwrap()).unwrap();

        filter_link
            .filtered_payload_buffers
            .iter()
            .for_each(|payload| {
                println!("Payload size: {}", payload.len());
            });

        let rdh2 = RdhCRUv7::load(&mut buf_reader).unwrap();
        rdh2.print();
    }
}

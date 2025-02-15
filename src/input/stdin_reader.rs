//! Wrapper for a reader, implements [BufferedReaderWrapper].
//!
//! The wrapper can wrap both [std::io::BufReader] and [StdInReaderSeeker].
//! Needed because [std::io::Stdin] does not implement seek_relative, and this serves as a convenient way to skip unwanted data.
//! seek_relative is used to skip over unwanted bytes in the input stream, such as links unwanted by the user
use super::bufreader_wrapper::BufferedReaderWrapper;
use std::io::{self, Read, SeekFrom};

/// Wrapper for a reader where input data can be read from, implements [BufferedReaderWrapper].
pub struct StdInReaderSeeker<R> {
    /// Generic reader that is wrapped
    pub reader: R,
}

/// Specialization for [std::io::Stdin]
impl BufferedReaderWrapper for StdInReaderSeeker<std::io::Stdin> {
    fn seek_relative(&mut self, offset: i64) -> io::Result<()> {
        // Seeking is not supported in stdin, so we have to read the bytes and discard them
        let mut buf = vec![0; offset as usize];
        std::io::stdin().lock().read_exact(&mut buf)?;
        Ok(())
    }
}

impl io::Read for StdInReaderSeeker<std::io::Stdin> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.reader.lock().read(buf)
    }
}
impl io::Seek for StdInReaderSeeker<std::io::Stdin> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match pos {
            SeekFrom::Start(_) => Err(io::Error::new(
                io::ErrorKind::Other,
                "Cannot seek from start in stdin",
            )),
            SeekFrom::Current(_) => Err(io::Error::new(
                io::ErrorKind::Other,
                "Cannot seek from current in stdin, use seek_relative instead",
            )),
            SeekFrom::End(_) => Err(io::Error::new(
                io::ErrorKind::Other,
                "Cannot seek from end in stdin",
            )),
        }
    }
}

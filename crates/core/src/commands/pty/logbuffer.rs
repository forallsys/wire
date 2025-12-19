// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2024-2025 wire Contributors

/// Split into its own struct to be tested nicer
pub(crate) struct LogBuffer {
    buffer: Vec<u8>,
}

impl LogBuffer {
    pub const fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn process_slice(&mut self, slice: &[u8]) {
        self.buffer.extend_from_slice(slice);
    }

    pub fn next_line(&mut self) -> Option<Vec<u8>> {
        let line_end = self.buffer.iter().position(|x| *x == b'\n')?;

        let drained = self.buffer.drain(..line_end).collect();
        self.buffer.remove(0);
        Some(drained)
    }

    #[cfg(test)]
    fn take_lines(&mut self) -> Vec<Vec<u8>> {
        let mut lines = vec![];

        while let Some(line) = self.next_line() {
            lines.push(line);
        }

        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_line_processing() {
        let mut log_buffer = LogBuffer::new();

        log_buffer.process_slice(b"Writing key KeySpec { destination: \"/et");
        log_buffer.process_slice(b"c/keys/buildbot.aws.key\", user: \"buildbot\", group: \"buildbot-worker\", permissions: 384, length: 32, last: false, crc: 1370815231 }, 32 bytes of data");
        log_buffer.process_slice(b"\n");
        log_buffer.process_slice(b"xxx");
        log_buffer.process_slice(b"xx_WIRE");
        log_buffer.process_slice(b"_QUIT\n");
        let lines = log_buffer.take_lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            String::from_utf8_lossy(lines.first().unwrap()),
            "Writing key KeySpec { destination: \"/etc/keys/buildbot.aws.key\", user: \"buildbot\", group: \"buildbot-worker\", permissions: 384, length: 32, last: false, crc: 1370815231 }, 32 bytes of data"
        );
        assert_eq!(lines.get(1), Some(&"xxxxx_WIRE_QUIT".as_bytes().to_vec()));

        // taking leaves none
        assert_eq!(log_buffer.take_lines().len(), 0);
    }
}

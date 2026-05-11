#[derive(Debug, Clone)]
pub struct TranscriptBuffer {
    limit: usize,
    head: Vec<u8>,
    tail: Vec<u8>,
    total: usize,
}

impl TranscriptBuffer {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            head: Vec::new(),
            tail: Vec::new(),
            total: 0,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len();
        let head_limit = self.limit / 2;
        let tail_limit = self.limit - head_limit;

        if self.head.len() < head_limit {
            let take = (head_limit - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..take]);
        }

        if tail_limit == 0 {
            self.tail.clear();
            return;
        }

        if bytes.len() >= tail_limit {
            self.tail = bytes[bytes.len() - tail_limit..].to_vec();
            return;
        }

        let overflow = self
            .tail
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(tail_limit);
        if overflow > 0 {
            self.tail.drain(..overflow);
        }
        self.tail.extend_from_slice(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::TranscriptBuffer;

    #[test]
    fn transcript_tail_capacity_stays_at_tail_limit_for_large_chunk() {
        let mut transcript = TranscriptBuffer::new(1024);
        let bytes = vec![b'x'; 4096];

        transcript.push(&bytes);

        assert_eq!(transcript.head.len(), 512);
        assert_eq!(transcript.tail.len(), 512);
        assert!(
            transcript.tail.capacity() <= 512,
            "tail capacity exceeded tail limit: {}",
            transcript.tail.capacity()
        );
        assert_eq!(transcript.total, 4096);
    }

    #[test]
    fn transcript_tail_never_exceeds_tail_limit_across_small_chunks() {
        let mut transcript = TranscriptBuffer::new(10);

        transcript.push(b"0123");
        transcript.push(b"4567");
        transcript.push(b"89ab");

        assert_eq!(transcript.head, b"01234");
        assert_eq!(transcript.tail, b"789ab");
        assert_eq!(transcript.tail.len(), 5);
        assert_eq!(transcript.total, 12);
    }

    #[test]
    fn transcript_tail_handles_zero_limit_without_growth() {
        let mut transcript = TranscriptBuffer::new(0);

        transcript.push(b"abcdef");

        assert!(transcript.head.is_empty());
        assert!(transcript.tail.is_empty());
        assert_eq!(transcript.total, 6);
    }
}

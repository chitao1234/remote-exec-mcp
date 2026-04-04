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

        self.tail.extend_from_slice(bytes);
        if self.tail.len() > tail_limit {
            let drop_len = self.tail.len() - tail_limit;
            self.tail.drain(..drop_len);
        }
    }

    #[allow(dead_code)]
    pub fn render(&self) -> String {
        let mut data = self.head.clone();
        if self.total > self.limit {
            data.extend_from_slice(b"\n...<truncated>...\n");
        }
        data.extend_from_slice(&self.tail);
        String::from_utf8_lossy(&data).into_owned()
    }
}

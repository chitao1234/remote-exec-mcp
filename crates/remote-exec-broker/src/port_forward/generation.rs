#[derive(Debug, Clone)]
pub(crate) struct StreamIdAllocator {
    next: u32,
    step: u32,
    exhausted: bool,
}

impl StreamIdAllocator {
    pub(crate) fn new_odd() -> Self {
        Self {
            next: 1,
            step: 2,
            exhausted: false,
        }
    }

    pub(crate) fn new_odd_from(start: u32) -> Self {
        Self {
            next: start,
            step: 2,
            exhausted: false,
        }
    }

    pub(crate) fn next(&mut self) -> Option<u32> {
        if self.exhausted {
            return None;
        }
        let value = self.next;
        match self.next.checked_add(self.step) {
            Some(next) if next.checked_add(self.step).is_some() => self.next = next,
            _ => self.exhausted = true,
        }
        Some(value)
    }

    pub(crate) fn needs_generation_rotation(&self) -> bool {
        self.exhausted
    }

    #[cfg(test)]
    pub(crate) fn set_next_for_test(&mut self, next: u32) {
        self.next = next;
        self.exhausted = false;
    }
}

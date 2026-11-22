// SPDX-License-Identifier: MIT OR Apache-2.0

//! Fixed-capacity deferred kernel work.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
	Probe {
		source_cpu: usize,
		sequence: u64,
	},
}

pub struct WorkQueue<const N: usize> {
	entries: [Option<Work>; N],
	head: usize,
	length: usize,
	rejected: u64,
}

impl<const N: usize> WorkQueue<N> {
	pub const fn new() -> Self {
		Self {
			entries: [None; N],
			head: 0,
			length: 0,
			rejected: 0,
		}
	}

	pub fn push(&mut self, work: Work) -> Result<(), &'static str> {
		if self.length == N {
			self.rejected = self.rejected.saturating_add(1);
			return Err("Kernel work queue is full");
		}

		let tail = (self.head + self.length) % N;
		self.entries[tail] = Some(work);
		self.length += 1;
		Ok(())
	}

	pub fn pop(&mut self) -> Option<Work> {
		if self.length == 0 {
			return None;
		}

		let work = self.entries[self.head]
			.take()
			.expect("Occupied kernel work slot is empty");

		self.head = (self.head + 1) % N;
		self.length -= 1;
		Some(work)
	}

	pub fn rejected(&self) -> u64 {
		self.rejected
	}
}

//! A process's output as the daemon keeps it (MC-3): the last bytes, at most a cap, in chunks that each know where in the stream
//! they start. The stream's position is a byte sequence (`offset`) that only grows, counted over both of a run's streams (stdout and
//! stderr, in the order they were read) or a terminal's one. A client remembers the end of what it took and asks for what came after;
//! [`Cursor`] takes what arrives so each byte goes on exactly once, however replay and live output overlap.

use std::collections::VecDeque;

/// Which of a process's streams a chunk came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Out,
    Err,
    /// A terminal's one stream.
    Pty,
}

impl Stream {
    pub fn name(self) -> &'static str {
        match self {
            Stream::Out => "out",
            Stream::Err => "err",
            Stream::Pty => "pty",
        }
    }

    pub fn parse(text: &str) -> Option<Stream> {
        match text {
            "out" => Some(Stream::Out),
            "err" => Some(Stream::Err),
            "pty" => Some(Stream::Pty),
            _ => None,
        }
    }
}

/// One piece of output and where in the stream it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub offset: u64,
    pub stream: Stream,
    pub data: Vec<u8>,
}

impl Chunk {
    pub fn end(&self) -> u64 {
        self.offset + self.data.len() as u64
    }
}

/// What [`OutputRing::read_from`] gives: the bytes dropped before what is still kept (when the reader asked for older ones), the
/// chunks from there on (at most the bytes asked), and where the next read starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadFrom {
    pub lost: u64,
    pub chunks: Vec<Chunk>,
    pub next: u64,
}

/// The last `capacity` bytes of a process's output.
#[derive(Debug, Clone)]
pub struct OutputRing {
    capacity: usize,
    max_chunk: usize,
    chunks: VecDeque<Chunk>,
    size: usize,
    end: u64,
}

impl OutputRing {
    /// A ring of at most `capacity` bytes whose chunks are at most `max_chunk` bytes each.
    pub fn new(capacity: usize, max_chunk: usize) -> OutputRing {
        OutputRing { capacity: capacity.max(1), max_chunk: max_chunk.max(1), chunks: VecDeque::new(), size: 0, end: 0 }
    }

    /// The offset just past the last byte ever pushed.
    pub fn end(&self) -> u64 {
        self.end
    }

    /// The offset of the oldest byte still kept (the end when nothing is).
    pub fn start(&self) -> u64 {
        self.chunks.front().map_or(self.end, |chunk| chunk.offset)
    }

    /// The bytes kept now.
    pub fn len(&self) -> usize {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Adds `bytes` read from `stream`; gives the offset they start at. Older bytes go once the ring holds more than its capacity,
    /// and what is kept then starts at a character: the continuation bytes a dropped chunk's character left behind go too, so a
    /// reader that missed the start never sees half a character.
    pub fn push(&mut self, stream: Stream, bytes: &[u8]) -> u64 {
        let offset = self.end;
        for piece in bytes.chunks(self.max_chunk) {
            self.chunks.push_back(Chunk { offset: self.end, stream, data: piece.to_vec() });
            self.end += piece.len() as u64;
            self.size += piece.len();
        }
        self.trim();
        offset
    }

    fn trim(&mut self) {
        let mut dropped = false;
        while self.size > self.capacity {
            let Some(front) = self.chunks.front_mut() else { break };
            let over = self.size - self.capacity;
            if over >= front.data.len() {
                self.size -= front.data.len();
                self.chunks.pop_front();
            } else {
                front.data.drain(..over);
                front.offset += over as u64;
                self.size -= over;
            }
            dropped = true;
        }
        if dropped && let Some(front) = self.chunks.front_mut() {
            let continuation = front.data.iter().take(3).take_while(|b| (0x80..0xC0).contains(*b)).count();
            if continuation > 0 && continuation < front.data.len() {
                front.data.drain(..continuation);
                front.offset += continuation as u64;
                self.size -= continuation;
            } else if continuation > 0 {
                self.size -= front.data.len();
                self.chunks.pop_front();
            }
        }
    }

    /// The kept bytes from `from` on, at most about `max_bytes` of them (always at least one chunk when there is one): a `from` older
    /// than what is kept reads from the oldest kept byte and says how many were lost; one inside a chunk reads that chunk from there.
    pub fn read_from(&self, from: u64, max_bytes: usize) -> ReadFrom {
        let start = self.start();
        let (lost, mut at) = if from < start { (start - from, start) } else { (0, from.min(self.end)) };
        let mut chunks = Vec::new();
        let mut taken = 0usize;
        // The first chunk that ends after `at` (chunks are in order, so a binary search finds it).
        let first = self.chunks.partition_point(|chunk| chunk.end() <= at);
        for chunk in self.chunks.iter().skip(first) {
            if !chunks.is_empty() && taken + chunk.data.len() > max_bytes {
                break;
            }
            let skip = (at.saturating_sub(chunk.offset)) as usize;
            let data = chunk.data[skip.min(chunk.data.len())..].to_vec();
            taken += data.len();
            at = chunk.end();
            chunks.push(Chunk { offset: chunk.offset + skip as u64, stream: chunk.stream, data });
        }
        ReadFrom { lost, chunks, next: at }
    }
}

/// What [`Cursor::accept`] took from an arriving chunk: the bytes the reader never had (none when it had them all) and how many it
/// can never have (dropped before it asked).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted<'a> {
    pub lost: u64,
    pub data: &'a [u8],
}

/// Where a reader is in a process's stream: the end of the last byte it took. Replayed and live chunks may overlap or repeat; each
/// byte is taken once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub next: u64,
}

impl Cursor {
    pub fn at(next: u64) -> Cursor {
        Cursor { next }
    }

    /// Takes the chunk at `offset`: the part after what was taken before, and the gap before it when it starts later.
    pub fn accept<'a>(&mut self, offset: u64, data: &'a [u8]) -> Accepted<'a> {
        let end = offset + data.len() as u64;
        if end <= self.next {
            return Accepted { lost: 0, data: &[] };
        }
        let (lost, skip) = if offset > self.next { (offset - self.next, 0) } else { (0, (self.next - offset) as usize) };
        self.next = end;
        Accepted { lost, data: &data[skip..] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(read: &ReadFrom) -> Vec<u8> {
        read.chunks.iter().flat_map(|c| c.data.clone()).collect()
    }

    #[test]
    fn the_ring_keeps_its_last_bytes_with_their_offsets() {
        let mut ring = OutputRing::new(10, 4);
        assert_eq!(ring.push(Stream::Out, b"abcdef"), 0);
        assert_eq!(ring.push(Stream::Err, b"gh"), 6);
        assert_eq!((ring.start(), ring.end(), ring.len()), (0, 8, 8));
        let all = ring.read_from(0, usize::MAX);
        assert_eq!(bytes(&all), b"abcdefgh");
        assert_eq!(
            all.chunks.iter().map(|c| (c.offset, c.stream)).collect::<Vec<_>>(),
            [(0, Stream::Out), (4, Stream::Out), (6, Stream::Err)]
        );
        assert_eq!((all.lost, all.next), (0, 8));
        ring.push(Stream::Out, b"ijkl");
        assert_eq!((ring.start(), ring.end(), ring.len()), (2, 12, 10), "the oldest two went");
        let from_zero = ring.read_from(0, usize::MAX);
        assert_eq!((from_zero.lost, bytes(&from_zero)), (2, b"cdefghijkl".to_vec()));
        let mid = ring.read_from(5, usize::MAX);
        assert_eq!((mid.lost, bytes(&mid)), (0, b"fghijkl".to_vec()), "a read inside a chunk starts there");
        assert_eq!(ring.read_from(12, usize::MAX).chunks, vec![], "nothing after the end");
        assert_eq!(ring.read_from(99, usize::MAX).next, 12, "a read past the end waits at the end");
    }

    #[test]
    fn a_read_is_bounded_but_always_moves() {
        let mut ring = OutputRing::new(100, 4);
        ring.push(Stream::Pty, &[b'x'; 20]);
        let first = ring.read_from(0, 6);
        assert_eq!((bytes(&first).len(), first.next), (4, 4), "a second chunk would pass the bound");
        let tiny = ring.read_from(0, 1);
        assert_eq!(tiny.next, 4, "at least one chunk");
        let mut at = 0;
        let mut total = 0;
        while at < ring.end() {
            let read = ring.read_from(at, 8);
            total += bytes(&read).len();
            at = read.next;
        }
        assert_eq!(total, 20);
    }

    #[test]
    fn what_is_kept_never_starts_inside_a_character() {
        // "가" is EA B0 80; the ring keeps 5 bytes, so trimming lands inside the second one.
        let mut ring = OutputRing::new(5, 64);
        ring.push(Stream::Pty, "가가가".as_bytes());
        let read = ring.read_from(0, usize::MAX);
        assert_eq!(String::from_utf8(bytes(&read)).unwrap(), "가", "the half character went with the rest");
        assert_eq!(read.lost, 6);
        assert_eq!(ring.start(), 6);
        // A chunk of nothing but continuation bytes after a trim goes whole.
        let mut ring = OutputRing::new(4, 2);
        ring.push(Stream::Pty, &[b'a', b'b', 0xEA, 0xB0, 0x80, b'c']);
        let read = ring.read_from(0, usize::MAX);
        assert!(std::str::from_utf8(&bytes(&read)).is_ok(), "{:?}", bytes(&read));
    }

    #[test]
    fn a_cursor_takes_each_byte_once_across_replay_and_live_output() {
        let mut ring = OutputRing::new(1 << 20, 3);
        let text = "안녕 hello 세계";
        ring.push(Stream::Out, text.as_bytes());
        let mut cursor = Cursor::default();
        let mut seen = Vec::new();
        // The live chunk arrives first (subscribed before the replay), then the replay repeats it, then a chunk overlaps.
        let live = ring.read_from(6, usize::MAX);
        let replay = ring.read_from(0, usize::MAX);
        for chunk in replay.chunks.iter() {
            let accepted = cursor.accept(chunk.offset, &chunk.data);
            assert_eq!(accepted.lost, 0);
            seen.extend_from_slice(accepted.data);
        }
        for chunk in live.chunks.iter() {
            assert!(cursor.accept(chunk.offset, &chunk.data).data.is_empty(), "already taken");
        }
        let overlap = cursor.accept(text.len() as u64 - 2, b"\x95\x8cmore");
        assert_eq!(overlap.data, b"more");
        seen.extend_from_slice(overlap.data);
        assert_eq!(String::from_utf8(seen).unwrap(), format!("{text}more"));
        // A chunk that starts later than the cursor says what was lost.
        let mut cursor = Cursor::at(10);
        let gap = cursor.accept(15, b"xy");
        assert_eq!((gap.lost, gap.data, cursor.next), (5, &b"xy"[..], 17));
    }

    #[test]
    fn a_partial_character_at_a_cursor_split_reassembles() {
        let text = "가나다".as_bytes();
        let mut ring = OutputRing::new(1 << 20, 64);
        ring.push(Stream::Pty, text);
        // The reader took the first four bytes (one and a third characters) before it went away.
        let mut taken = text[..4].to_vec();
        let mut cursor = Cursor::at(4);
        let replay = ring.read_from(cursor.next, usize::MAX);
        for chunk in &replay.chunks {
            taken.extend_from_slice(cursor.accept(chunk.offset, &chunk.data).data);
        }
        assert_eq!(String::from_utf8(taken).unwrap(), "가나다");
    }

    #[test]
    fn a_push_bigger_than_the_ring_keeps_its_end() {
        let mut ring = OutputRing::new(8, 4);
        ring.push(Stream::Out, b"0123456789abcdef");
        assert_eq!((ring.start(), ring.end(), ring.len()), (8, 16, 8));
        assert_eq!(bytes(&ring.read_from(0, usize::MAX)), b"89abcdef");
        assert!(Stream::parse("pty") == Some(Stream::Pty) && Stream::parse("x").is_none());
        assert_eq!(Stream::Err.name(), "err");
    }
}

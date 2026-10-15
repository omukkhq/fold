use std::fs;
use std::io;
use std::path::Path;

const SEED: u64 = 0x666f_6c64_2d62_656e;
const MIN_SIZE: u64 = 4 * 1024;
const SIZE_SPAN: u64 = 32 * 1024;

/// How big the fixtures are. `smoke` divides every count by 100, so the harness runs in seconds on any machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sizes {
    pub files: usize,
    pub directories: usize,
    pub proposal_files: usize,
    pub idle_files: usize,
}

impl Sizes {
    pub const FULL: Sizes = Sizes {
        files: 10_000,
        directories: 100,
        proposal_files: 50,
        idle_files: 100,
    };

    pub fn smoke() -> Sizes {
        let divide = |count: usize| (count / 100).max(1);
        Sizes {
            files: divide(Self::FULL.files),
            directories: divide(Self::FULL.directories),
            proposal_files: divide(Self::FULL.proposal_files),
            idle_files: divide(Self::FULL.idle_files),
        }
    }
}

/// Splitmix64: small, seeded, and the same on every platform.
struct Generator(u64);

impl Generator {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^ (mixed >> 31)
    }
}

/// The path of file `index`, spread over `directories` folders.
pub fn path_of(index: usize, directories: usize) -> String {
    format!("dir{:03}/file{index:05}.txt", index % directories.max(1))
}

/// 4 KiB to 36 KiB of hexadecimal text lines, identical on every run for the same index.
pub fn content_of(index: usize) -> Vec<u8> {
    let mut generator = Generator(SEED ^ (index as u64).wrapping_mul(0xd6e8_feb8_6659_fd93));
    let size = usize::try_from(MIN_SIZE + generator.next() % (SIZE_SPAN + 1)).unwrap_or(0);
    let mut bytes = Vec::with_capacity(size + 80);
    while bytes.len() < size {
        for _ in 0..4 {
            bytes.extend_from_slice(format!("{:016x}", generator.next()).as_bytes());
        }
        bytes.push(b'\n');
    }
    bytes.truncate(size - 1);
    bytes.push(b'\n');
    bytes
}

/// Writes `files` files under `root` and returns their total size in bytes.
pub fn write_repository(root: &Path, files: usize, directories: usize) -> io::Result<u64> {
    let mut total = 0;
    for index in 0..files {
        let path = root.join(path_of(index, directories));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = content_of(index);
        total += content.len() as u64;
        fs::write(path, content)?;
    }
    Ok(total)
}

/// Adds a line at the top of a file, a change the merge of an append at the bottom keeps apart from.
pub fn prepend_line(path: &Path, line: &str) -> io::Result<()> {
    let content = fs::read(path)?;
    let mut changed = format!("{line}\n").into_bytes();
    changed.extend_from_slice(&content);
    fs::write(path, changed)
}

/// Adds a line at the bottom of a file.
pub fn append_line(path: &Path, line: &str) -> io::Result<()> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new().append(true).open(path)?;
    writeln!(file, "{line}")
}

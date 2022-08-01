//! Parity hash experiments
//!
//! Using hash parity to eliminate the extra writes needed due to swapping in MCUboot.
//!
//! Credits:
//! - Brendan Moran came up with the idea of using parity to recover when one page has been lost.
//! - Geraint Luff suggested using a Merkle tree to find the recovery point
//! - David Brown came up with the idea of detecting the recovery point instead of storing the
//!   state, as well as the importance of skipping pages where the two images have identical
//!   contents.

// TODO: Implement skip map, and store it.

// Turn this off once more code is written.
#![allow(dead_code)]

use sha2::{
    Digest, Sha256,
};
use std::{fmt, io::Write, str};

fn main() {
    let mut work = Status::new(6);
    // println!("p: {:#?}", work);

    work.swap();
    work.final_check();
}

/// All flash operations happen in terms of a given page size.  The page will be at least as large
/// as the larger of the erasable and writable units of the flash device.  When the two slots are
/// on different flash devices, it will need to be at least as large as the largest of the two
/// devices erasable and writable units.
///
/// For this test framework, the page size will be a compile-time constant, although MCUboot itself
/// will support this being only a maximum, and a smaller actual page size being used.  With the
/// parity hash algorithm, each slot must be 2 pages larger than the largest image (including
/// TLV/manifest) that can be placed in the slot.  TODO: it might be possible to only need one
/// extra, if we can get away with only having parity for slot 1.
const PAGE_SIZE: usize = 32;

/// A slot consists of a number of pages of data.  The slot will have a size that is larger than
/// the image.  There is also metadata associated with the slot about how large the image is and
/// such.
#[derive(Debug)]
struct Slot {
    data: Vec<Page>,
}

/// A single page is some amount of data.
#[derive(Debug)]
struct Page {
    payload: Vec<u8>,
    pstate: PageState,
}

/// Flash state of a given page.  The idea is to fail an operation if the page is partially
/// operated on, and we make use of the data in it.
#[derive(Debug, Eq, PartialEq)]
enum PageState {
    Written,
    Erased,
    PartiallyWritten,
    PartiallyErased,
}

struct PageLocation {
    slot: usize,
    index: usize,
}

/// For this experiment, we don't try to map the status into the flash itself, but merely store it
/// in memory.
#[derive(Debug)]
struct Status {
    slot0: Slot,
    slot1: Slot,
    root: Vec<u8>,
    parity: Vec<u8>,
}

impl Page {
    /// Construct a new, empty page.  If 'init' is set, the parameters are used to seed the page
    /// with appropriate data.
    fn new(init: Option<PageLocation>) -> Page {
        let mut buf = vec![0xFFu8; PAGE_SIZE];

        if let Some(loc) = init {
            let mut writer: &mut [u8] = &mut buf;

            write!(writer, "Slot {}, page {}, data", loc.slot, loc.index).unwrap();
            Page {
                payload: buf,
                pstate: PageState::Written,
            }
        } else {
            Page {
                payload: buf,
                pstate: PageState::Erased,
            }
        }
    }

    /// Verify that the contents of a page is as expected.
    fn check(&self, loc: PageLocation) {
        // TODO: This isn't very efficient.
        let expected = Page::new(Some(loc));

        if self.pstate != expected.pstate {
            panic!("Page state is incorrect");
        }

        if &self.payload[..] != &expected.payload[..] {
            panic!("Page contents is incorrect");
        }
    }

    /// Compute the digest of the given page.
    fn digest(&self) -> Vec<u8> {
        let mut md = Sha256::new();
        md.update(&self.payload);
        md.finalize().to_vec()
    }

    /// Normal read from the page. If the page is not in a state where this makes sense, it will
    /// abort with a failure.
    fn read(&self, buffer: &mut [u8]) {
        match self.pstate {
            PageState::Written => {
                buffer.copy_from_slice(&self.payload);
            }
            _ => panic!("Invalid state for read"),
        }
    }

    /// Safe read from the page. Reads from flash without regard to the state. We should never
    /// depend on the value read here.
    fn read_whatever(&self, buffer: &mut [u8]) {
        buffer.copy_from_slice(&self.payload);
    }

    /// Erase the contents of the page.
    fn erase(&mut self) {
        self.pstate = PageState::Erased;
        self.payload.fill(0xFF);
    }

    /// Write new contents to the page.  Will panic if the data isn't freshly erased.
    fn write(&mut self, buffer: &[u8]) {
        if let PageState::Erased = self.pstate {
            self.payload.copy_from_slice(buffer);
            self.pstate = PageState::Written;
        } else {
            panic!("Attempt to write to unerased flash page");
        }
    }
}

// Nice display for Page.
impl fmt::Display for Page {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(pos) = self.payload.iter().position(|&x| x == 0xFF) {
            write!(f, "[{}]", str::from_utf8(&self.payload[0..pos]).unwrap())?;
        } else {
            write!(f, "[unknown page]")?;
        }
        Ok(())
    }
}

impl Slot {
    fn new(slot: usize, pages: usize) -> Slot {
        let data = (0 .. pages).map(|i| {
            Page::new(Some(PageLocation { slot, index: i }))
        }).collect();
        Slot { data }
    }

    /// Compute the Merkel root for the data in the slot.
    /// TODO: Don't return and copy result, twice
    fn compute_root(&self) -> Vec<u8> {
        let mut state = Sha256::new();
        for b in &self.data {
            state.update(&b.digest());
        }
        state.finalize().to_vec()
    }

    /// Compute a parity block for the entire image.
    fn compute_parity(&self) -> Vec<u8> {
        let mut result = vec![0u8; PAGE_SIZE];

        for b in &self.data {
            for (i, &bt) in b.payload.iter().enumerate() {
                result[i] ^= bt;
            }
        }
        result
    }
}

impl Status {
    fn new(size: usize) -> Status {
        let slot0 = Slot::new(0, size);
        let slot1 = Slot::new(1, size);
        let root = slot1.compute_root();
        let parity = slot0.compute_parity();

        Status { slot0, slot1, root, parity }
    }

    fn swap(&mut self) {
        // TODO: Support different sizes for the slots.
        assert_eq!(self.slot0.data.len(), self.slot1.data.len());

        // We need two buffers for the operation.
        let mut abuf = vec![0u8; PAGE_SIZE];
        let mut bbuf = vec![0u8; PAGE_SIZE];

        for sec in 0 .. self.slot0.data.len() {
            let slot0 = &mut self.slot0.data[sec];
            let slot1 = &mut self.slot1.data[sec];

            slot0.read(&mut abuf);
            slot1.read(&mut bbuf);

            slot0.erase();
            slot0.write(&mut bbuf);
            slot1.erase();
            slot1.write(&mut abuf);
        }
    }

    /// Compute a final check to ensure that the given swap has completed.
    fn final_check(&self) {
        for sec in 0 .. self.slot0.data.len() {
            self.slot0.data[sec].check(PageLocation { slot: 1, index: sec });
            self.slot1.data[sec].check(PageLocation { slot: 0, index: sec });
        }
    }
}

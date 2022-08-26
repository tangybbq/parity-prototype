//! Simulated flash memory.
//!
//! Simulates a flash memory.  Imposes very strict constraints to try and capture the worst case of
//! all of the flash memory types we will support.
//!
//! This flash memory is divided into "pages", which correspond to the largest-smallest unit that
//! can be erased in flash.  Real flash will be treated as a subset of these.
//!
//! There are two types of flash memory supported: NOR-type where erases are large, but writes are
//! fairly small (8 bytes or less), and page-based, where writes also have to be done in larger
//! units.  The "page" will correspond to erases in either of these, but the small-write will
//! enable some optimizations where status can be written incrementally in the last page instead of
//! requiring its own page(s).

use crate::Result;
use anyhow::anyhow;
use std::io::Write;

/// For this prototype, we will make the page size a compile-time constant.  This can be abstracted
/// later, if this code is ever used in a real device.
pub const PAGE_SIZE: usize = 32;

/// The flash consists of a number of pages of data.  In this usage, we will treat each partition
/// as just a different flash device.
#[derive(Debug)]
pub struct Slot {
    data: Vec<Page>,
}

impl Slot {
    pub fn new(pages: usize) -> Slot {
        let data: Vec<_> = (0..pages).map(|_p| Page::new()).collect();
        Slot { data }
    }
}

/// A page itself is some amount of data.
#[derive(Debug)]
pub struct Page {
    payload: Vec<u8>,
    pstate: PageState,
}

/// The state of a given page.
#[derive(Debug, Eq, PartialEq)]
enum PageState {
    Written,
    Erased,
    PartiallyWritten,
    PartiallyErased,
}

impl Page {
    /// Construct a new, empty page.  It is erased, but set as partially erased to ensure actual
    /// erases happen before it is used.
    fn new() -> Page {
        let buf = vec![0xFFu8; PAGE_SIZE];
        Page {
            payload: buf,
            pstate: PageState::PartiallyErased,
        }
    }

    /// A utility, to fill a page buffer with the expected data for a page.
    fn fill(buf: &mut [u8], slot: usize, index: usize) {
        assert_eq!(buf.len(), PAGE_SIZE, "Page size is not correct");
        buf.fill(0xFF);
        let mut writer: &mut [u8] = buf;
        write!(writer, "Slot {}, page {}, data", slot, index).unwrap();
    }

    /// Check a filled page.
    fn check(buf: &[u8], slot: usize, index: usize) -> Result<()> {
        assert_eq!(buf.len(), PAGE_SIZE, "Page size is not correct");
        let mut tmp = vec![0xFFu8; PAGE_SIZE];
        Self::fill(&mut tmp, slot, index);
        if buf == tmp {
            Ok(())
        } else {
            Err(anyhow!("Page mismatch"))
        }
    }

    /// Normal read from the page.  If the page is not in a state where this makes sense, it will
    /// return an error.
    fn read(&self, buffer: &mut [u8]) -> Result<()> {
        match self.pstate {
            PageState::Written => {
                buffer.copy_from_slice(&self.payload);
                Ok(())
            }
            _ => Err(anyhow!("Read from invalid state: {:?}", self.pstate)),
        }
    }

    /// A safe read from the page.  Reads from flash without regard to the state.  Nothing should
    /// depend on the value read here, but is needed when we don't know where an operation left
    /// off.
    fn read_whatever(&self, buffer: &mut [u8]) -> Result<()> {
        buffer.copy_from_slice(&self.payload);
        Ok(())
    }

    /// Erase the contents of the page.
    fn erase(&mut self) -> Result<()> {
        self.pstate = PageState::Erased;
        self.payload.fill(0xFF);
        Ok(())
    }

    /// Partial erase.  We make no changes to the data, acting as if we are at the very beginning
    /// of the operation.
    fn partial_erase(&mut self) {
        self.pstate = PageState::PartiallyErased;
    }

    /// Write new contents to the page.  Will error if the page isn't in the erased state.
    fn write(&mut self, buffer: &[u8]) -> Result<()> {
        if let PageState::Erased = self.pstate {
            self.payload.copy_from_slice(buffer);
            self.pstate = PageState::Written;
            Ok(())
        } else {
            Err(anyhow!(
                "Attempt to write to unerased page {:?}",
                self.pstate
            ))
        }
    }
}

#[test]
fn test_flash_basics() {
    let mut fl = Slot::new(10);
    let mut buf = vec![0u8; PAGE_SIZE];

    // Ensure that these pages are all in a weird erased state.
    for p in 0..fl.data.len() {
        assert!(matches!(fl.data[p].read(&mut buf), Err(_)));
    }

    // It should also be an error to write to the pages.
    assert!(matches!(fl.data[1].write(&buf), Err(_)));

    // Erase the page, and make sure it is usable that way.
    assert!(matches!(fl.data[1].erase(), Ok(())));

    // Read should still fail.
    assert!(matches!(fl.data[1].read(&mut buf), Err(_)));

    // But read whatever should return erased data.
    assert!(matches!(fl.data[1].read_whatever(&mut buf), Ok(())));

    // The data should appear erased.
    assert!(buf.iter().all(|&b| b == 0xFF));

    Page::fill(&mut buf, 5, 7);

    // Write the pattern to the erased page.
    assert!(matches!(fl.data[1].write(&buf), Ok(())));

    buf.fill(0x00);

    // Read it back.
    assert!(matches!(fl.data[1].read(&mut buf), Ok(())));

    assert!(matches!(Page::check(&buf, 5, 7), Ok(())));
}
